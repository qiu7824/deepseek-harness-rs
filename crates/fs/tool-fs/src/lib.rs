//! Model-facing `read`, `write`, and `edit` tools over `ctx.fs`.

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc, downcast_arc};
use dsh_fs::{
    FileSystem, FsEditGuard, FsEditRequest, FsError, FsErrorCode, FsInfoType, FsObservation,
    FsTarget, FsWriteIntent,
};
use dsh_fs_observation_policy::FsObservationActorHandle;
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{
    FileDiff, FileLocation, ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition,
    ToolExecution, ToolOutputDefinition, ToolResultView,
};
use futures::{FutureExt, StreamExt};
use std::sync::Arc;

pub const NAME: &str = "tool-fs";
pub const INJECT: [&str; 3] = ["tools", "fs", "systemPrompt"];
const READ_LIMIT: u64 = 2000;
const READ_MAX_LINE_LENGTH: usize = 2000;
const READ_MAX_BYTES: usize = 50 * 1024;
const STREAM_MIN_SIZE: u64 = 10 * 1024 * 1024;

fn body_error(error: FsError) -> ToolBodyError {
    let remedy = match error.code {
        FsErrorCode::FsStaleVersion => " — re-read the file, then retry",
        FsErrorCode::FsNotObserved => " — read the file, then retry",
        _ => "",
    };
    ToolBodyError {
        message: format!("{error}{remedy}"),
        info: Some(dsh_tools::ToolErrorInfo {
            name: "FsError".into(),
            code: error.code.as_str().into(),
        }),
    }
}

fn actor(exec: &ToolExecution) -> FsObservationActorHandle {
    FsObservationActorHandle {
        session_key: exec.agent.as_ref().map(|a| a.session().identity()),
    }
}

fn cwd(exec: &ToolExecution) -> Option<String> {
    exec.agent
        .as_ref()
        .and_then(|a| a.session().header().cwd.clone())
}

fn signal(exec: &ToolExecution) -> dsh_fs::AbortPredicate {
    exec.signal.lock().clone()
}

async fn target(
    fs: &Arc<dyn FileSystem>,
    path: &str,
    exec: &ToolExecution,
) -> Result<FsTarget, ToolBodyError> {
    if path.trim().is_empty() {
        return Err(ToolBodyError::plain("file_path must be a non-empty string"));
    }
    let abort = signal(exec);
    fs.resolve(
        path,
        Some(&dsh_fs::ResolveOptions {
            cwd: cwd(exec),
            signal: Some(abort),
        }),
    )
    .await
    .map_err(body_error)
}

fn emit_observed(
    ctx: &Context,
    target: &FsTarget,
    observation: FsObservation,
    exec: &ToolExecution,
) {
    ctx.emit(
        "fs/observed",
        vec![arc(target.clone()), arc(observation), arc(actor(exec))],
    );
}

async fn write_intent(
    ctx: &Context,
    target: &FsTarget,
    exec: &ToolExecution,
) -> Option<FsWriteIntent> {
    let value = ctx
        .waterfall(
            "fs/write-intent",
            vec![arc(target.clone()), arc(actor(exec))],
            Box::pin(async { arc(()) }),
        )
        .await;
    downcast_arc::<FsWriteIntent>(&value).map(|v| v.as_ref().clone())
}

async fn edit_intent(
    ctx: &Context,
    target: &FsTarget,
    exec: &ToolExecution,
) -> Result<Option<FsEditGuard>, ToolBodyError> {
    let future = ctx.waterfall(
        "fs/edit-intent",
        vec![arc(target.clone()), arc(actor(exec))],
        Box::pin(async { arc(()) }),
    );
    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
        Ok(value) => Ok(downcast_arc::<FsEditGuard>(&value).map(|v| v.as_ref().clone())),
        Err(value) => match value.downcast::<FsError>() {
            Ok(error) => Err(body_error(*error)),
            Err(_) => Err(ToolBodyError::plain(
                "tool pipeline panicked while resolving the edit intent",
            )),
        },
    }
}

fn output_object(
    render: impl Fn(
        &serde_json::Value,
        &serde_json::Value,
    ) -> Result<Vec<dsh_llm::ContentBlock>, String>
    + Send
    + Sync
    + 'static,
    schema: serde_json::Value,
    presentation_meta: Option<
        Arc<
            dyn Fn(&serde_json::Value, &serde_json::Value) -> Result<serde_json::Value, String>
                + Send
                + Sync,
        >,
    >,
) -> ToolOutputDefinition {
    ToolOutputDefinition {
        schema,
        render: Arc::new(render),
        presentation_meta,
    }
}

struct Service {
    ctx: Context,
    fs: Arc<dyn FileSystem>,
}

impl Service {
    fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        let fs = ctx
            .get_typed::<Arc<dyn FileSystem>>("fs", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs requires the fs service")?;
        let tools = ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs requires the tools service")?;
        let prompt = ctx
            .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs requires systemPrompt")?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            fs,
        });
        prompt.section(ctx, PromptSection { name:"tool:read".into(), order:100.0, text:PromptText::Static("Use the read tool — not shell commands like cat — to inspect text files. Results include line numbers. Use offset and limit to continue reading large files.".into()), complete:None });
        prompt.section(ctx, PromptSection { name:"tool:write".into(), order:101.0, text:PromptText::Static("Use the write tool to create files or completely replace file contents. Existing files are overwritten, so read an existing file first and prefer edit for targeted changes.".into()), complete:None });
        prompt.section(ctx, PromptSection { name:"tool:edit".into(), order:102.0, text:PromptText::Static("Use the edit tool for targeted changes to existing UTF-8 text files. It replaces literal old_string with new_string; by default old_string must appear exactly once.".into()), complete:None });
        tools.register(ctx, service.read_definition())?;
        tools.register(ctx, service.write_definition())?;
        tools.register(ctx, service.edit_definition())?;
        Ok(service)
    }

    fn read_definition(self: &Arc<Self>) -> ToolDefinition {
        let service = self.clone();
        ToolDefinition {
            name: "read".into(),
            description: "Read a UTF-8 text file and return line-numbered content.".into(),
            parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"file_path":{"type":"string"},"offset":{"type":"number"},"limit":{"type":"number"}},"required":["file_path"]}),
            output: output_object(
                |args, value| {
                    let offset = value["offset"].as_u64().unwrap_or(1);
                    let total = value["totalLines"].as_u64().unwrap_or(0);
                    let rows = value["lines"].as_array().cloned().unwrap_or_default();
                    let end = rows
                        .last()
                        .and_then(|v| v["number"].as_u64())
                        .unwrap_or(offset.saturating_sub(1));
                    let body = rows
                        .iter()
                        .map(|v| format!("{}: {}", v["number"], v["text"].as_str().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let requested = args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(READ_LIMIT);
                    let footer = if rows.len() < requested as usize && end < total {
                        format!(
                            "(Output capped. Showing lines {offset}-{end}. Use offset={} to continue.)",
                            end + 1
                        )
                    } else if end < total {
                        format!(
                            "(Showing lines {offset}-{end} of {total}. Use offset={} to continue.)",
                            end + 1
                        )
                    } else {
                        format!("(End of file - total {total} lines)")
                    };
                    let content = if body.is_empty() {
                        footer
                    } else {
                        format!("{body}\n\n{footer}")
                    };
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: format!(
                            "<path>{}</path>\n<type>file</type>\n<content>\n{}\n</content>",
                            value["path"].as_str().unwrap_or(""),
                            content
                        ),
                    }])
                },
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"offset":{"type":"integer"},"lines":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"number":{"type":"integer"},"text":{"type":"string"}},"required":["number","text"]}},"totalLines":{"type":"integer"}},"required":["path","offset","lines","totalLines"]}),
                None,
            ),
            timeout_ms: None,
            is_concurrency_safe: Some(Arc::new(|_| true)),
            execute: Arc::new(move |args, run| {
                let s = service.clone();
                let args = args.clone();
                let exec = run.execution.clone();
                Box::pin(async move {
                    let path = args
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolBodyError::plain("file_path is required"))?;
                    let integer = |name: &str, default: u64| -> Result<u64, ToolBodyError> {
                        match args.get(name) {
                            None => Ok(default),
                            Some(v) => v.as_u64().filter(|v| *v > 0).ok_or_else(|| {
                                ToolBodyError::plain(format!("{name} must be a positive integer"))
                            }),
                        }
                    };
                    let offset = integer("offset", 1)?;
                    let limit = integer("limit", READ_LIMIT)?;
                    if limit > READ_LIMIT {
                        return Err(ToolBodyError::plain(format!(
                            "limit must be less than or equal to {READ_LIMIT}"
                        )));
                    }
                    let target = target(&s.fs, path, &exec).await?;
                    let signal = exec.signal.lock().clone();
                    let info =
                        s.fs.stat(&target, Some(signal.clone()))
                            .await
                            .map_err(body_error)?;
                    let Some(info) = info else {
                        emit_observed(&s.ctx, &target, FsObservation::Absent, &exec);
                        return Err(body_error(FsError::new(
                            format!("cannot read \"{}\": not found", target.display_path),
                            FsErrorCode::FsNotFound,
                        )));
                    };
                    if info.kind != FsInfoType::File {
                        return Err(body_error(FsError::new(
                            format!(
                                "cannot read \"{}\": not a regular file",
                                target.display_path
                            ),
                            FsErrorCode::FsNotRegularFile,
                        )));
                    }
                    let content =
                        if info.size.is_none() || info.size.unwrap_or(0) >= STREAM_MIN_SIZE {
                            let mut stream =
                                s.fs.stream_text(&target, Some(signal))
                                    .await
                                    .map_err(body_error)?;
                            let mut out = String::new();
                            while let Some(chunk) = stream.next().await {
                                out.push_str(&chunk.map_err(body_error)?);
                            }
                            out
                        } else {
                            s.fs.read_text(&target, Some(signal))
                                .await
                                .map_err(body_error)?
                        };
                    let mut all: Vec<&str> = content.split_terminator('\n').collect();
                    if content.is_empty() {
                        all.clear();
                    }
                    let total = all.len() as u64;
                    if offset > total && !(total == 0 && offset == 1) {
                        return Err(body_error(FsError::new(
                            format!(
                                "offset {offset} is out of range for \"{}\" ({total} lines)",
                                target.display_path
                            ),
                            FsErrorCode::FsNotFound,
                        )));
                    }
                    let mut bytes = 0usize;
                    let mut lines = Vec::new();
                    for (i, raw) in all
                        .iter()
                        .enumerate()
                        .skip((offset - 1) as usize)
                        .take(limit as usize)
                    {
                        let raw = raw.strip_suffix('\r').unwrap_or(raw);
                        let mut text: String = raw.chars().take(READ_MAX_LINE_LENGTH + 1).collect();
                        if text.chars().count() > READ_MAX_LINE_LENGTH {
                            text = format!(
                                "{}... (line truncated to {READ_MAX_LINE_LENGTH} chars)",
                                text.chars().take(READ_MAX_LINE_LENGTH).collect::<String>()
                            );
                        }
                        let extra = text.len() + usize::from(!lines.is_empty());
                        if bytes + extra > READ_MAX_BYTES {
                            break;
                        }
                        bytes += extra;
                        lines.push(serde_json::json!({"number":i+1,"text":text}));
                    }
                    emit_observed(
                        &s.ctx,
                        &target,
                        FsObservation::Present {
                            version: info.version,
                        },
                        &exec,
                    );
                    Ok(
                        serde_json::json!({"path":target.display_path,"offset":offset,"lines":lines,"totalLines":total}),
                    )
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|args| {
                Some(ToolCallView::Generic {
                    title: format!("Read {}", args["file_path"].as_str().unwrap_or("")),
                    kind: Some(ToolCallKind::Read),
                    raw_input: None,
                    content: None,
                    locations: Some(vec![FileLocation {
                        path: args["file_path"].as_str().unwrap_or("").into(),
                        line: args.get("offset").and_then(|v| v.as_u64()).or(Some(1)),
                    }]),
                })
            })),
            present_result: None,
        }
    }

    fn write_definition(self: &Arc<Self>) -> ToolDefinition {
        let s = self.clone();
        ToolDefinition {
            name: "write".into(),
            description: "Create or fully replace a UTF-8 text file.".into(),
            parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}),
            output: output_object(
                |_, v| {
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: format!(
                            "<path>{}</path>\n<type>file</type>\n<content>\n{} file\n</content>",
                            v["path"].as_str().unwrap_or(""),
                            if v["operation"] == "create" {
                                "Created"
                            } else {
                                "Updated"
                            }
                        ),
                    }])
                },
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"operation":{"type":"string","enum":["create","update"]},"before":{"oneOf":[{"type":"string"},{"type":"null"}]},"after":{"type":"string"}},"required":["path","operation","before","after"]}),
                Some(Arc::new(|_args, value| Ok(value.clone()))),
            ),
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, run| {
                let s = s.clone();
                let a = args.clone();
                let e = run.execution.clone();
                Box::pin(async move {
                    let p = a["file_path"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("file_path is required"))?;
                    let c = a["content"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("content is required"))?;
                    let t = target(&s.fs, p, &e).await?;
                    let intent = write_intent(&s.ctx, &t, &e).await;
                    let o =
                        s.fs.write_text(&t, c, intent.as_ref(), Some(signal(&e)), None)
                            .await
                            .map_err(body_error)?;
                    emit_observed(
                        &s.ctx,
                        &t,
                        FsObservation::Present { version: o.version },
                        &e,
                    );
                    Ok(
                        serde_json::json!({"path":t.display_path,"operation":match o.operation{dsh_fs::FsWriteOperation::Create=>"create",dsh_fs::FsWriteOperation::Update=>"update"},"before":o.before,"after":o.after}),
                    )
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|a| {
                Some(ToolCallView::Diff {
                    title: format!("Write {}", a["file_path"].as_str().unwrap_or("")),
                    diffs: vec![FileDiff {
                        path: a["file_path"].as_str().unwrap_or("").into(),
                        old_text: None,
                        new_text: a["content"].as_str().unwrap_or("").into(),
                    }],
                    locations: Some(vec![FileLocation {
                        path: a["file_path"].as_str().unwrap_or("").into(),
                        line: None,
                    }]),
                })
            })),
            present_result: Some(Arc::new(|_args, result| {
                if result.is_error {
                    return None;
                }
                let value = result.meta.as_ref()?;
                Some(ToolResultView::Diff {
                    title: value["path"].as_str().map(|path| format!("Write {path}")),
                    diffs: vec![FileDiff {
                        path: value["path"].as_str().unwrap_or("").into(),
                        old_text: value["before"].as_str().map(str::to_string),
                        new_text: value["after"].as_str().unwrap_or("").into(),
                    }],
                })
            })),
        }
    }

    fn edit_definition(self: &Arc<Self>) -> ToolDefinition {
        let s = self.clone();
        ToolDefinition {
            name: "edit".into(),
            description: "Edit an existing UTF-8 text file by replacing literal text.".into(),
            parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}),
            output: output_object(
                |a, v| {
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: if a
                            .get("replace_all")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            format!(
                                "The file {} has been updated. All occurrences were successfully replaced.",
                                v["path"].as_str().unwrap_or("")
                            )
                        } else {
                            format!(
                                "The file {} has been updated successfully.",
                                v["path"].as_str().unwrap_or("")
                            )
                        },
                    }])
                },
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"before":{"type":"string"},"after":{"type":"string"}},"required":["path","before","after"]}),
                Some(Arc::new(|_args, value| Ok(value.clone()))),
            ),
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args, run| {
                let s = s.clone();
                let a = args.clone();
                let e = run.execution.clone();
                Box::pin(async move {
                    let p = a["file_path"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("file_path is required"))?;
                    let old = a["old_string"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("old_string is required"))?;
                    let new = a["new_string"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("new_string is required"))?;
                    if old.is_empty() {
                        return Err(ToolBodyError::plain(
                            "old_string must be a non-empty string",
                        ));
                    }
                    if old == new {
                        return Err(ToolBodyError::plain(
                            "old_string and new_string must differ",
                        ));
                    }
                    let t = target(&s.fs, p, &e).await?;
                    let guard = edit_intent(&s.ctx, &t, &e).await?;
                    let o =
                        s.fs.edit_text(
                            &t,
                            &FsEditRequest {
                                old_string: old.into(),
                                new_string: new.into(),
                                replace_all: a
                                    .get("replace_all")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                            },
                            guard.as_ref(),
                            Some(signal(&e)),
                            None,
                        )
                        .await
                        .map_err(body_error)?;
                    emit_observed(
                        &s.ctx,
                        &t,
                        FsObservation::Present { version: o.version },
                        &e,
                    );
                    Ok(serde_json::json!({"path":t.display_path,"before":o.before,"after":o.after}))
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|a| {
                Some(ToolCallView::Diff {
                    title: format!("Edit {}", a["file_path"].as_str().unwrap_or("")),
                    diffs: vec![FileDiff {
                        path: a["file_path"].as_str().unwrap_or("").into(),
                        old_text: a["old_string"].as_str().map(str::to_string),
                        new_text: a["new_string"].as_str().unwrap_or("").into(),
                    }],
                    locations: Some(vec![FileLocation {
                        path: a["file_path"].as_str().unwrap_or("").into(),
                        line: None,
                    }]),
                })
            })),
            present_result: Some(Arc::new(|_args, result| {
                if result.is_error {
                    return None;
                }
                let value = result.meta.as_ref()?;
                Some(ToolResultView::Diff {
                    title: value["path"].as_str().map(|path| format!("Edit {path}")),
                    diffs: vec![FileDiff {
                        path: value["path"].as_str().unwrap_or("").into(),
                        old_text: value["before"].as_str().map(str::to_string),
                        new_text: value["after"].as_str().unwrap_or("").into(),
                    }],
                })
            })),
        }
    }
}

pub struct ToolFsPlugin;
#[async_trait::async_trait]
impl Plugin for ToolFsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }
    async fn apply(&self, ctx: &Context, _: ArcValue) -> Result<(), PluginError> {
        Service::install(ctx)
            .map(|_| ())
            .map_err(|e| PluginError::from(anyhow::anyhow!(e)))
    }
}
