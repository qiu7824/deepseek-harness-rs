//! Model-facing `str_replace_editor` over the Harness filesystem seam.
//! Rust port of `packages/fs/tool-str-replace-editor/src/index.ts`.
//!
//! # Deviations
//!
//! - `FsError` rejections from the `fs/edit-intent` waterfall travel as
//!   panics (the Rust waterfall has no error channel); the tool catches
//!   them and restores the structured `{ name, code }` error info.
//! - The config schema is a schemastery schema validated by the plugin
//!   form; `apply` performs the cross-field checks itself.
//! - Command internals receive the owned `Arc<ToolExecution>` (the
//!   `&ToolRunContext` reference cannot cross the `'static` body future).

use std::sync::Arc;

use cordis::{
    ArcValue, BoxFuture, Context, InjectSpec, Plugin, PluginError, ValidationError, arc, downcast,
    downcast_arc,
};
use dsh_fs::{
    AbortPredicate, FileSystem, FsEditGuard, FsError, FsErrorCode, FsInfo, FsObservation, FsTarget,
    FsWriteIntent,
};
use dsh_fs_observation_policy::FsObservationActorHandle;
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode, sandbox_denial_marker};
use dsh_sandbox_policy::{SandboxPolicyRequest, SandboxPolicyService};
use dsh_schemastery::{Data, Schema};
use dsh_tools::{
    FileDiff, FileLocation, ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition,
    ToolErrorInfo, ToolExecution, ToolOutputDefinition, ToolRunContext,
};

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "tool-str-replace-editor";

/// Services required before the plugin can register the tool.
pub const INJECT: [&str; 2] = ["tools", "fs"];

const TRUNCATED_MESSAGE: &str = "<response clipped><NOTE>To save on context only part of this file has been shown to you. You should retry this tool after you have searched inside the file with `grep -n` in order to find the line numbers of what you are looking for.</NOTE>";

const DEFAULT_DESCRIPTION: &str = "\
Custom editing tool for viewing, creating and editing files
* State is persistent across command calls and discussions with the user
* If `path` is a file, `view` displays the result of applying `cat -n`. If `path` is a directory, `view` lists non-hidden files and directories up to 2 levels deep
* The `create` command cannot be used if the specified `path` already exists as a file
* If a `command` generates a long output, it will be truncated and marked with `<response clipped>`

Notes for using the `str_replace` command:
* The `old_str` parameter should match EXACTLY one or more consecutive lines from the original file. Be mindful of whitespaces!
* If the `old_str` parameter is not unique in the file, the replacement will not be performed. Make sure to include enough context in `old_str` to make it unique
* The `new_str` parameter should contain the edited lines that should replace the `old_str`";

/// Configuration for the string-replacement editor tool.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Maximum returned view characters before clipping (default 16000).
    pub max_output_chars: Option<u64>,
    /// Model-facing tool description.
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    max_output_chars: u64,
    description: String,
}

fn maybe_truncate(content: &str, max_output_chars: u64) -> String {
    if content.chars().count() as u64 <= max_output_chars {
        return content.to_string();
    }
    let clipped: String = content.chars().take(max_output_chars as usize).collect();
    format!("{clipped}{TRUNCATED_MESSAGE}")
}

/// The tool error channel for one `FsError` (the TS structured
/// `{ name, code }` info).
fn fs_tool_error(error: &FsError) -> ToolBodyError {
    ToolBodyError {
        message: error.to_string(),
        info: Some(ToolErrorInfo {
            name: "FsError".to_string(),
            code: error.code.as_str().to_string(),
        }),
    }
}

/// One execution's observed-state actor identity (the TS
/// `FsObservationActor`; the session's opaque identity key).
fn actor_of(execution: &Arc<ToolExecution>) -> FsObservationActorHandle {
    FsObservationActorHandle {
        session_key: execution
            .agent
            .as_ref()
            .map(|agent| agent.session().identity()),
    }
}

/// How the mounted filesystem's confinement feeds the per-call policy
/// (TS `MutationPolicy`).
struct MutationPolicy {
    policy: Option<Arc<SandboxPolicyService>>,
}

impl MutationPolicy {
    fn new(ctx: &Context, fs: &Arc<dyn FileSystem>) -> Result<Self, String> {
        let policy = if fs.sandbox_mode().is_some() {
            Some(
                ctx.get_typed::<Arc<SandboxPolicyService>>("sandboxPolicy", false)
                    .map(|slot| slot.as_ref().clone())
                    .ok_or_else(|| {
                        "tool-str-replace-editor: the mounted filesystem confines but ctx.sandboxPolicy is missing"
                            .to_string()
                    })?,
            )
        } else {
            None
        };
        Ok(Self { policy })
    }

    fn resolve(&self, execution: &Arc<ToolExecution>) -> Option<SandboxExecutionPolicy> {
        self.policy.as_ref().map(|policy| {
            policy.resolve(&SandboxPolicyRequest {
                session: execution
                    .agent
                    .as_ref()
                    .map(|agent| Arc::new(agent.session().clone())),
                mode: None,
            })
        })
    }

    fn map_error(&self, error: FsError, policy: Option<&SandboxExecutionPolicy>) -> FsError {
        if error.code != FsErrorCode::FsSandboxDenied {
            return error;
        }
        let mode = policy
            .map(|policy| policy.mode)
            .unwrap_or(SandboxMode::ReadOnly);
        FsError::with_cause(
            sandbox_denial_marker(mode),
            FsErrorCode::FsSandboxDenied,
            Box::new(error),
        )
    }
}

async fn resolve_target(
    fs: &Arc<dyn FileSystem>,
    path: &str,
    signal: AbortPredicate,
) -> Result<FsTarget, ToolBodyError> {
    if path.trim().is_empty() {
        return Err(ToolBodyError::plain("path must be a non-empty string"));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(ToolBodyError::plain(format!(
            "The path {path} is not an absolute path, it should start with `/`. Maybe you meant /{path}?"
        )));
    }
    fs.resolve(
        path,
        Some(&dsh_fs::ResolveOptions {
            cwd: None,
            signal: Some(signal),
        }),
    )
    .await
    .map_err(|error| fs_tool_error(&error))
}

async fn stat_existing(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    target: &FsTarget,
    command: &str,
    execution: &Arc<ToolExecution>,
) -> Result<FsInfo, ToolBodyError> {
    let signal = execution.signal.lock().clone();
    let info = fs
        .stat(target, Some(signal))
        .await
        .map_err(|error| fs_tool_error(&error))?;
    let Some(info) = info else {
        ctx.emit(
            "fs/observed",
            vec![
                arc(target.clone()),
                arc(FsObservation::Absent),
                arc(actor_of(execution)),
            ],
        );
        return Err(fs_tool_error(&FsError::new(
            format!(
                "The path {} does not exist. Please provide a valid path.",
                target.display_path
            ),
            FsErrorCode::FsNotFound,
        )));
    };
    if info.kind == dsh_fs::FsInfoType::Directory && command != "view" {
        return Err(fs_tool_error(&FsError::new(
            format!(
                "The path {} is a directory and only the `view` command can be used on directories",
                target.display_path
            ),
            FsErrorCode::FsNotRegularFile,
        )));
    }
    Ok(info)
}

fn required_for_command(
    value: Option<&str>,
    parameter: &str,
    command: &str,
    allow_empty: bool,
) -> Result<String, ToolBodyError> {
    let Some(value) = value else {
        return Err(ToolBodyError::plain(format!(
            "Parameter `{parameter}` is required for command: {command}"
        )));
    };
    if !allow_empty && value.is_empty() {
        return Err(ToolBodyError::plain(format!(
            "Parameter `{parameter}` is empty for command: {command}"
        )));
    }
    Ok(value.to_string())
}

/// The line-numbered content view (TS `formatFileView`).
fn format_file_view(
    path: &str,
    content: &str,
    max_output_chars: u64,
    view_range: Option<&[serde_json::Value]>,
) -> Result<String, ToolBodyError> {
    let all_lines: Vec<&str> = content.split('\n').collect();
    let mut initial_line = 1usize;
    let mut prompt = format!(
        "Here's the content of {path} with line numbers (which has a total of {} lines)",
        all_lines.len()
    );
    let lines: &[&str] = if let Some(view_range) = view_range {
        if view_range.len() != 2 {
            return Err(ToolBodyError::plain(
                "Invalid `view_range`. It should be a list of two integers.",
            ));
        }
        let (Some(requested_initial), Some(requested_final)) =
            (view_range[0].as_i64(), view_range[1].as_i64())
        else {
            return Err(ToolBodyError::plain(
                "Invalid `view_range`. It should be a list of two integers.",
            ));
        };
        initial_line = requested_initial as usize;
        if initial_line < 1 || initial_line > all_lines.len() {
            return Err(ToolBodyError::plain(format!(
                "Invalid `view_range`: [{requested_initial}, {requested_final}]. Its first element `{requested_initial}` should be within the range of lines of the file: [1, {}]",
                all_lines.len()
            )));
        }
        if requested_final > all_lines.len() as i64 {
            return Err(ToolBodyError::plain(format!(
                "Invalid `view_range`: [{requested_initial}, {requested_final}]. Its second element `{requested_final}` should be smaller than the number of lines in the file: `{}`",
                all_lines.len()
            )));
        }
        if requested_final != -1 && requested_final < requested_initial {
            return Err(ToolBodyError::plain(format!(
                "Invalid `view_range`: [{requested_initial}, {requested_final}]. Its second element `{requested_final}` should be larger or equal than its first `{requested_initial}`"
            )));
        }
        prompt += &format!(" with view_range=[{initial_line}, {requested_final}]");
        if requested_final == -1 {
            &all_lines[initial_line - 1..]
        } else {
            &all_lines[initial_line - 1..requested_final as usize]
        }
    } else {
        all_lines.as_slice()
    };
    let numbered: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:>6}  {line}", initial_line + index))
        .collect();
    Ok(maybe_truncate(
        &format!("{prompt}:\n{}\n", numbered.join("\n")),
        max_output_chars,
    ))
}

async fn list_directory(
    fs: &Arc<dyn FileSystem>,
    target: &FsTarget,
    max_output_chars: u64,
    execution: &Arc<ToolExecution>,
) -> Result<String, ToolBodyError> {
    fn visit(
        fs: Arc<dyn FileSystem>,
        dir: FsTarget,
        depth: u64,
        execution: Arc<ToolExecution>,
    ) -> BoxFuture<'static, Result<Vec<String>, ToolBodyError>> {
        Box::pin(async move {
            let signal = execution.signal.lock().clone();
            let entries = fs
                .list_dir(&dir, Some(signal))
                .await
                .map_err(|error| fs_tool_error(&error))?;
            let mut rows: Vec<String> = Vec::new();
            for entry in entries.iter().filter(|candidate| {
                !candidate.name.starts_with('.')
                    && candidate.name != "node_modules"
                    && candidate.name != "__pycache__"
            }) {
                let kind = match entry.kind {
                    dsh_fs::FsInfoType::Directory => "d",
                    dsh_fs::FsInfoType::File => "f",
                    dsh_fs::FsInfoType::Other => "?",
                };
                rows.push(format!("{kind}\t{}", entry.target.display_path));
                if entry.kind == dsh_fs::FsInfoType::Directory && depth < 2 {
                    rows.extend(
                        visit(
                            fs.clone(),
                            entry.target.clone(),
                            depth + 1,
                            execution.clone(),
                        )
                        .await?,
                    );
                }
            }
            Ok(rows)
        })
    }

    let mut rows = vec![format!("d\t{}", target.display_path)];
    rows.extend(visit(fs.clone(), target.clone(), 1, execution.clone()).await?);
    rows.sort_by(|left, right| {
        let left_path = left.split_once('\t').map(|(_, path)| path).unwrap_or("");
        let right_path = right.split_once('\t').map(|(_, path)| path).unwrap_or("");
        left_path.cmp(right_path)
    });
    let listing = maybe_truncate(&format!("{}\n", rows.join("\n")), max_output_chars);
    Ok(format!(
        "Here're the files and directories up to 2 levels deep in {}, excluding hidden items, node_modules, and Python cache directories:\n{listing}\n",
        target.display_path
    ))
}

async fn view_path(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    path: &str,
    view_range: Option<&[serde_json::Value]>,
    max_output_chars: u64,
    execution: &Arc<ToolExecution>,
) -> Result<String, ToolBodyError> {
    let signal = execution.signal.lock().clone();
    let target = resolve_target(fs, path, signal).await?;
    let info = stat_existing(ctx, fs, &target, "view", execution).await?;
    if info.kind == dsh_fs::FsInfoType::Directory {
        if view_range.is_some() {
            return Err(ToolBodyError::plain(
                "The `view_range` parameter is not allowed when `path` points to a directory.",
            ));
        }
        return list_directory(fs, &target, max_output_chars, execution).await;
    }
    if info.kind != dsh_fs::FsInfoType::File {
        return Err(fs_tool_error(&FsError::new(
            format!(
                "cannot view \"{}\": not a regular file or directory",
                target.display_path
            ),
            FsErrorCode::FsNotRegularFile,
        )));
    }
    let signal = execution.signal.lock().clone();
    let content = fs
        .read_text(&target, Some(signal))
        .await
        .map_err(|error| fs_tool_error(&error))?;
    ctx.emit(
        "fs/observed",
        vec![
            arc(target.clone()),
            arc(FsObservation::Present {
                version: info.version.clone(),
            }),
            arc(actor_of(execution)),
        ],
    );
    format_file_view(&target.display_path, &content, max_output_chars, view_range)
}

/// Resolve the `fs/write-intent` waterfall with the observation-policy
/// fallback (the TS `createIfAbsent` default).
async fn write_intent(
    ctx: &Context,
    target: &FsTarget,
    actor: FsObservationActorHandle,
) -> FsWriteIntent {
    let value = ctx
        .waterfall(
            "fs/write-intent",
            vec![arc(target.clone()), arc(actor)],
            Box::pin(async { arc(FsWriteIntent::CreateIfAbsent) }),
        )
        .await;
    downcast_arc::<FsWriteIntent>(&value)
        .map(|slot| slot.as_ref().clone())
        .unwrap_or(FsWriteIntent::CreateIfAbsent)
}

/// Resolve the `fs/edit-intent` waterfall; an `FsError` rejection travels
/// as a panic and is restored to the structured error channel.
async fn edit_intent(
    ctx: &Context,
    target: &FsTarget,
    actor: FsObservationActorHandle,
) -> Result<Option<FsEditGuard>, ToolBodyError> {
    let future = ctx.waterfall(
        "fs/edit-intent",
        vec![arc(target.clone()), arc(actor)],
        Box::pin(async { arc(()) }),
    );
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(future)).await {
        Ok(value) => Ok(downcast_arc::<FsEditGuard>(&value).map(|slot| slot.as_ref().clone())),
        Err(payload) => match payload.downcast::<FsError>() {
            Ok(error) => Err(fs_tool_error(&error)),
            Err(_) => Err(ToolBodyError::plain(
                "tool pipeline panicked while resolving the edit intent",
            )),
        },
    }
}

async fn create_file(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    policy: &MutationPolicy,
    path: &str,
    file_text: Option<&str>,
    execution: &Arc<ToolExecution>,
) -> Result<String, ToolBodyError> {
    let content = required_for_command(file_text, "file_text", "create", true)?;
    let sandbox_policy = policy.resolve(execution);
    let signal = execution.signal.lock().clone();
    let target = resolve_target(fs, path, signal.clone()).await?;
    if fs
        .stat(&target, Some(signal.clone()))
        .await
        .map_err(|error| fs_tool_error(&error))?
        .is_some()
    {
        return Err(ToolBodyError::plain(format!(
            "File already exists at: {}. Cannot overwrite files using command `create`.",
            target.display_path
        )));
    }
    let intent = write_intent(ctx, &target, actor_of(execution)).await;
    let outcome = match fs
        .write_text(
            &target,
            &content,
            Some(&intent),
            Some(signal),
            sandbox_policy.as_ref(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(fs_tool_error(
                &policy.map_error(error, sandbox_policy.as_ref()),
            ));
        }
    };
    ctx.emit(
        "fs/observed",
        vec![
            arc(target.clone()),
            arc(FsObservation::Present {
                version: outcome.version.clone(),
            }),
            arc(actor_of(execution)),
        ],
    );
    Ok(format!(
        "New file created successfully at: {}",
        target.display_path
    ))
}

fn match_offsets(content: &str, search: &str) -> Vec<usize> {
    let mut offsets: Vec<usize> = Vec::new();
    let mut offset = 0;
    loop {
        let Some(match_offset) = content[offset..].find(search) else {
            return offsets;
        };
        let absolute = offset + match_offset;
        offsets.push(absolute);
        offset = absolute + search.len();
    }
}

fn line_numbers_at(content: &str, offsets: &[usize]) -> Vec<usize> {
    let mut line = 1usize;
    let mut cursor = 0usize;
    offsets
        .iter()
        .map(|offset| {
            while cursor < *offset {
                if content.as_bytes()[cursor] == b'\n' {
                    line += 1;
                }
                cursor += 1;
            }
            line
        })
        .collect()
}

async fn replace_in_file(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    policy: &MutationPolicy,
    path: &str,
    old_str: Option<&str>,
    new_str: Option<&str>,
    execution: &Arc<ToolExecution>,
) -> Result<String, ToolBodyError> {
    let sandbox_policy = policy.resolve(execution);
    let signal = execution.signal.lock().clone();
    let target = resolve_target(fs, path, signal.clone()).await?;
    let intent = edit_intent(ctx, &target, actor_of(execution)).await?;
    let old_value = required_for_command(old_str, "old_str", "str_replace", false)?;
    let new_value = new_str.unwrap_or_default();
    let info = stat_existing(ctx, fs, &target, "str_replace", execution).await?;
    if info.kind != dsh_fs::FsInfoType::File {
        return Err(fs_tool_error(&FsError::new(
            format!(
                "cannot edit \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::FsNotRegularFile,
        )));
    }
    let signal = execution.signal.lock().clone();
    let before = fs
        .read_text(&target, Some(signal))
        .await
        .map_err(|error| fs_tool_error(&error))?;
    let offsets = match_offsets(&before, &old_value);
    let Some(offset) = offsets.first().copied() else {
        return Err(fs_tool_error(&FsError::new(
            format!(
                "No replacement was performed, old_str `{old_value}` did not appear verbatim in {}.",
                target.display_path
            ),
            FsErrorCode::FsEditNotFound,
        )));
    };
    if offsets.len() > 1 {
        let lines = line_numbers_at(&before, &offsets);
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        return Err(fs_tool_error(&FsError::new(
            format!(
                "No replacement was performed. Multiple occurrences of old_str `{old_value}` in lines [{}]. Please ensure it is unique",
                lines.join(", ")
            ),
            FsErrorCode::FsAmbiguousEdit,
        )));
    }
    let after = format!(
        "{}{}{}",
        &before[..offset],
        new_value,
        &before[offset + old_value.len()..]
    );
    let expected = match &intent {
        Some(guard) => FsWriteIntent::ReplaceIfVersion {
            version: guard.version.clone(),
        },
        None => FsWriteIntent::ReplaceIfVersion {
            version: info.version.clone(),
        },
    };
    let signal = execution.signal.lock().clone();
    let outcome = match fs
        .write_text(
            &target,
            &after,
            Some(&expected),
            Some(signal),
            sandbox_policy.as_ref(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(fs_tool_error(
                &policy.map_error(error, sandbox_policy.as_ref()),
            ));
        }
    };
    ctx.emit(
        "fs/observed",
        vec![
            arc(target.clone()),
            arc(FsObservation::Present {
                version: outcome.version.clone(),
            }),
            arc(actor_of(execution)),
        ],
    );
    Ok(format!(
        "The file {} has been edited successfully.",
        target.display_path
    ))
}

async fn insert_in_file(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    policy: &MutationPolicy,
    path: &str,
    insert_line: Option<i64>,
    new_str: Option<&str>,
    execution: &Arc<ToolExecution>,
) -> Result<String, ToolBodyError> {
    let Some(insert_line) = insert_line else {
        return Err(ToolBodyError::plain(
            "Parameter `insert_line` is required for command: insert",
        ));
    };
    let value = required_for_command(new_str, "new_str", "insert", true)?;
    let sandbox_policy = policy.resolve(execution);
    let signal = execution.signal.lock().clone();
    let target = resolve_target(fs, path, signal.clone()).await?;
    let intent = edit_intent(ctx, &target, actor_of(execution)).await?;
    let info = stat_existing(ctx, fs, &target, "insert", execution).await?;
    if info.kind != dsh_fs::FsInfoType::File {
        return Err(fs_tool_error(&FsError::new(
            format!(
                "cannot insert into \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::FsNotRegularFile,
        )));
    }
    let signal = execution.signal.lock().clone();
    let before = fs
        .read_text(&target, Some(signal))
        .await
        .map_err(|error| fs_tool_error(&error))?;
    let lines: Vec<&str> = before.split('\n').collect();
    if insert_line < 0 || insert_line > lines.len() as i64 {
        return Err(ToolBodyError::plain(format!(
            "Invalid `insert_line` parameter: {insert_line}. It should be within the range of lines of the file: [0, {}]",
            lines.len()
        )));
    }
    let insert_line = insert_line as usize;
    // One join over the interleaved parts: lines before + the inserted
    // value's lines + lines after (the TS `[...a, ...b, ...c].join('\n')`).
    let mut parts: Vec<&str> = lines[..insert_line].to_vec();
    parts.extend(value.split('\n'));
    parts.extend(lines[insert_line..].iter().copied());
    let after = parts.join("\n");
    let expected = match &intent {
        Some(guard) => FsWriteIntent::ReplaceIfVersion {
            version: guard.version.clone(),
        },
        None => FsWriteIntent::ReplaceIfVersion {
            version: info.version.clone(),
        },
    };
    let signal = execution.signal.lock().clone();
    let outcome = match fs
        .write_text(
            &target,
            &after,
            Some(&expected),
            Some(signal),
            sandbox_policy.as_ref(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(fs_tool_error(
                &policy.map_error(error, sandbox_policy.as_ref()),
            ));
        }
    };
    ctx.emit(
        "fs/observed",
        vec![
            arc(target.clone()),
            arc(FsObservation::Present {
                version: outcome.version.clone(),
            }),
            arc(actor_of(execution)),
        ],
    );
    Ok(format!(
        "The file {} has been edited successfully.",
        target.display_path
    ))
}

/// The pending-call presentation (TS `presentEditorCall`).
fn present_editor_call(args: &serde_json::Value) -> Option<ToolCallView> {
    let command = args.get("command")?.as_str()?;
    let path = args.get("path")?.as_str()?.to_string();
    match command {
        "view" => Some(ToolCallView::Generic {
            title: format!("view {path}"),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: Some(vec![FileLocation { path, line: None }]),
        }),
        "create" => Some(ToolCallView::Diff {
            title: format!("create {path}"),
            diffs: vec![FileDiff {
                path: path.clone(),
                old_text: None,
                new_text: args
                    .get("file_text")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
            }],
            locations: Some(vec![FileLocation { path, line: None }]),
        }),
        "str_replace" => Some(ToolCallView::Diff {
            title: format!("str_replace {path}"),
            diffs: vec![FileDiff {
                path: path.clone(),
                old_text: args
                    .get("old_str")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
                new_text: args
                    .get("new_str")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
            }],
            locations: Some(vec![FileLocation { path, line: None }]),
        }),
        "insert" => Some(ToolCallView::Generic {
            title: format!("insert {path}"),
            kind: Some(ToolCallKind::Edit),
            raw_input: None,
            content: None,
            locations: Some(vec![FileLocation {
                path,
                line: args
                    .get("insert_line")
                    .and_then(|value| value.as_i64())
                    .map(|line| std::cmp::max(1, line + 1) as u64),
            }]),
        }),
        _ => None,
    }
}

/// The mounted tool: the `str_replace_editor` definition.
pub struct ToolStrReplaceEditorService {
    ctx: Context,
    fs: Arc<dyn FileSystem>,
    policy: MutationPolicy,
    config: ResolvedConfig,
}

impl ToolStrReplaceEditorService {
    /// Register the tool and return the owning service (TS `apply`).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let resolved = ResolvedConfig {
            max_output_chars: config.max_output_chars.unwrap_or(16_000),
            description: config
                .description
                .unwrap_or_else(|| DEFAULT_DESCRIPTION.to_string()),
        };
        if resolved.max_output_chars == 0 || resolved.max_output_chars > 9_007_199_254_740_991 {
            return Err(
                "tool-str-replace-editor: maxOutputChars must be a positive safe integer"
                    .to_string(),
            );
        }
        if resolved.description.trim().is_empty() {
            return Err("tool-str-replace-editor: description must be non-empty".to_string());
        }
        let fs = ctx
            .get_typed::<Arc<dyn FileSystem>>("fs", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-tool-str-replace-editor requires the fs service".to_string())?;
        let tools = ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-tool-str-replace-editor requires the tools service".to_string())?;
        let policy = MutationPolicy::new(ctx, &fs)?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            fs,
            policy,
            config: resolved,
        });
        tools.register(ctx, service.definition())?;
        Ok(service)
    }

    fn definition(self: &Arc<Self>) -> ToolDefinition {
        let service = self.clone();
        ToolDefinition {
            name: "str_replace_editor".to_string(),
            description: service.config.description.clone(),
            parameters: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "command": { "type": "string", "enum": ["view", "create", "str_replace", "insert"], "description": "The commands to run. Allowed options are: `view`, `create`, `str_replace`, `insert`." },
                    "path": { "type": "string", "description": "Absolute path to file or directory, e.g. `/repo/file.py` or `/repo`." },
                    "file_text": { "oneOf": [{ "type": "string" }, { "type": "null" }], "description": "Required string parameter of `create` command. A null placeholder is treated as omitted by commands that do not use it." },
                    "insert_line": { "oneOf": [{ "type": "integer" }, { "type": "null" }], "description": "Required integer parameter of `insert` command. A null placeholder is treated as omitted by commands that do not use it." },
                    "new_str": { "oneOf": [{ "type": "string" }, { "type": "null" }], "description": "Optional string parameter of `str_replace` and required string parameter of `insert`. Null is accepted only as an unused-command placeholder; omit this field to delete a str_replace match." },
                    "old_str": { "oneOf": [{ "type": "string" }, { "type": "null" }], "description": "Required string parameter of `str_replace`. A null placeholder is treated as omitted by commands that do not use it." },
                    "view_range": { "oneOf": [{ "type": "array", "items": { "type": "integer" } }, { "type": "null" }], "description": "Optional view line range. Omitted or null selects the full file." },
                },
                "required": ["command", "path"],
            }),
            output: ToolOutputDefinition {
                schema: serde_json::json!({ "type": "string" }),
                render: Arc::new(|_args, value| {
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: value.as_str().unwrap_or_default().to_string(),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: {
                let service = service.clone();
                Arc::new(move |args: &serde_json::Value, exec: &ToolRunContext| {
                    let service = service.clone();
                    let args = args.clone();
                    let execution = exec.execution.clone();
                    Box::pin(async move {
                        let ctx = service.ctx.clone();
                        let fs = service.fs.clone();
                        let command = args.get("command").and_then(|v| v.as_str());
                        let path = args.get("path").and_then(|v| v.as_str());
                        let Some(command) = command else {
                            return Err(ToolBodyError::plain("Parameter `command` is required"));
                        };
                        let path = path
                            .ok_or_else(|| ToolBodyError::plain("Parameter `path` is required"))?;
                        let view_range = args.get("view_range").and_then(|v| v.as_array());
                        let result: Result<String, ToolBodyError> = match command {
                            "view" => {
                                view_path(
                                    &ctx,
                                    &fs,
                                    path,
                                    view_range.map(|range| range.as_slice()),
                                    service.config.max_output_chars,
                                    &execution,
                                )
                                .await
                            }
                            "create" => {
                                create_file(
                                    &ctx,
                                    &fs,
                                    &service.policy,
                                    path,
                                    args.get("file_text").and_then(|v| v.as_str()),
                                    &execution,
                                )
                                .await
                            }
                            "str_replace" => {
                                if args.get("new_str").is_some_and(serde_json::Value::is_null) {
                                    return Err(ToolBodyError::plain(
                                        "Parameter `new_str` must be omitted or contain a string for command: str_replace",
                                    ));
                                }
                                replace_in_file(
                                    &ctx,
                                    &fs,
                                    &service.policy,
                                    path,
                                    args.get("old_str").and_then(|v| v.as_str()),
                                    args.get("new_str").and_then(|v| v.as_str()),
                                    &execution,
                                )
                                .await
                            }
                            "insert" => {
                                insert_in_file(
                                    &ctx,
                                    &fs,
                                    &service.policy,
                                    path,
                                    args.get("insert_line").and_then(|v| v.as_i64()),
                                    args.get("new_str").and_then(|v| v.as_str()),
                                    &execution,
                                )
                                .await
                            }
                            other => Err(ToolBodyError::plain(format!("Invalid command: {other}"))),
                        };
                        result.map(serde_json::Value::String)
                    })
                })
            },
            finalize_content: None,
            present_call: Some(Arc::new(present_editor_call)),
            present_result: None,
        }
    }
}

/// Register one `str_replace_editor` tool over `ctx.fs` (TS `apply`).
pub fn apply(ctx: &Context, config: Config) -> Result<(), String> {
    ToolStrReplaceEditorService::install(ctx, config).map(|_| ())
}

/// The plugin-config schema (TS `static Config`).
fn config_schema() -> Schema {
    Schema::object(indexmap::IndexMap::from([
        (
            "maxOutputChars".to_string(),
            Schema::number().min(1.0).default(Data::Number(16_000.0)),
        ),
        (
            "description".to_string(),
            Schema::string().default(Data::String(DEFAULT_DESCRIPTION.to_string())),
        ),
    ]))
}

fn data_from_json(value: &serde_json::Value) -> Data {
    match value {
        serde_json::Value::Null => Data::Null,
        serde_json::Value::Bool(value) => Data::Bool(*value),
        serde_json::Value::Number(value) => Data::Number(value.as_f64().unwrap_or(f64::NAN)),
        serde_json::Value::String(value) => Data::String(value.clone()),
        serde_json::Value::Array(items) => Data::Array(items.iter().map(data_from_json).collect()),
        serde_json::Value::Object(map) => Data::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), data_from_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod nullable_placeholder_tests {
    use super::*;

    fn test_service() -> Arc<ToolStrReplaceEditorService> {
        let ctx = Context::root();
        dsh_invariants::InvariantRegistry::new(
            &ctx,
            dsh_invariants::InvariantConfig {
                enabled: true,
                ..dsh_invariants::InvariantConfig::default()
            },
        );
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .expect("system prompt");
        dsh_tools::ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
        let root =
            std::env::temp_dir().join(format!("dsh-editor-null-schema-{}", uuid::Uuid::new_v4()));
        dsh_fs_local::LocalFileSystem::install(
            &ctx,
            dsh_fs_local::Config {
                cwd: Some(root.to_string_lossy().into_owned()),
                diff_basis_max_bytes: None,
            },
        )
        .expect("filesystem");
        ToolStrReplaceEditorService::install(&ctx, Config::default()).expect("editor service")
    }

    #[tokio::test]
    async fn editor_schema_accepts_null_for_command_specific_placeholders() {
        let service = test_service();
        let schema = &service.definition().parameters;

        for field in [
            "file_text",
            "insert_line",
            "new_str",
            "old_str",
            "view_range",
        ] {
            let variants = schema["properties"][field]["oneOf"]
                .as_array()
                .unwrap_or_else(|| panic!("{field} must expose oneOf"));
            assert!(
                variants.iter().any(
                    |variant| variant.get("type").and_then(|value| value.as_str()) == Some("null")
                ),
                "{field} must allow a null placeholder: {variants:#?}"
            );
        }
    }

    #[tokio::test]
    async fn null_placeholders_execute_only_when_unused_by_the_selected_command() {
        let service = test_service();
        let definition = service.definition();
        let tools = service
            .ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .expect("tools service")
            .as_ref()
            .clone();
        let path = std::env::temp_dir()
            .join(format!("dsh-editor-null-call-{}.txt", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        let created = tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("nullable-placeholders-create"),
                root_call_id: None,
                name: definition.name.clone(),
                arguments: serde_json::json!({
                    "command": "create",
                    "path": path,
                    "file_text": "hello",
                    "insert_line": null,
                    "new_str": null,
                    "old_str": null,
                    "view_range": null,
                }),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(
            !created.is_error,
            "unused null placeholders must execute: {:?}",
            created.error.as_ref().map(|error| error.message.as_str())
        );

        let invalid = tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("nullable-placeholders-invalid"),
                root_call_id: None,
                name: definition.name,
                arguments: serde_json::json!({
                    "command": "str_replace",
                    "path": std::env::temp_dir()
                        .join("dsh-editor-unused")
                        .to_string_lossy(),
                    "old_str": "hello",
                    "new_str": null,
                }),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(
            invalid
                .error
                .as_ref()
                .expect("str_replace new_str=null must fail")
                .message
                .contains("must be omitted or contain a string")
        );
    }
}

fn whole_number(value: &serde_json::Value) -> Option<u64> {
    if let Some(integer) = value.as_u64() {
        return Some(integer);
    }
    let number = value.as_f64()?;
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 {
        return None;
    }
    if number > 9_007_199_254_740_991.0 {
        return None;
    }
    Some(number as u64)
}

fn config_from_value(config: &ArcValue) -> Result<Config, String> {
    let Some(value) = downcast::<serde_json::Value>(config) else {
        return Ok(Config::default());
    };
    let max_output_chars = value
        .get("maxOutputChars")
        .map(|v| {
            whole_number(v).ok_or_else(|| {
                "tool-str-replace-editor: maxOutputChars must be a whole number".to_string()
            })
        })
        .transpose()?;
    let description = value
        .get("description")
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "tool-str-replace-editor: description must be a string".to_string())
        })
        .transpose()?;
    Ok(Config {
        max_output_chars,
        description,
    })
}

/// The Cordis plugin form (TS mounts the module with its schema).
pub struct ToolStrReplaceEditorPlugin;

impl ToolStrReplaceEditorPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for ToolStrReplaceEditorPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    fn validate(&self, config: ArcValue) -> Result<ArcValue, ValidationError> {
        let Some(value) = downcast::<serde_json::Value>(&config) else {
            return Ok(config);
        };
        let data = data_from_json(value);
        let validated = Schema::validate(&config_schema(), data)
            .map_err(|error| ValidationError::new([error.to_string()]))?;
        let json = validated
            .to_json()
            .unwrap_or_else(|| serde_json::Value::Null);
        Ok(arc(json))
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config_from_value(&config)
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        apply(ctx, config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
