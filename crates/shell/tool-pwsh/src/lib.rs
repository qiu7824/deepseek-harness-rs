use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_jobs::{JobHooks, JobOutcome, JobOutcomeStatus, JobRegistry, JobStart};
use dsh_llm::ContentBlock;
use dsh_sandbox_policy::{SandboxPolicyRequest, SandboxPolicyService};
use dsh_shell::{ShellExecRequest, ShellExecutor, ShellProcess, ShellProcessStatus};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use futures::future::BoxFuture;

mod native;
mod output;

fn shell_runtime_failure(message: String) -> ToolBodyError {
    for code in [
        "SANDBOX_UNAVAILABLE",
        "SANDBOX_SETUP_FAILED",
        "SANDBOX_SETUP_REQUIRED",
        "SANDBOX_SETUP_TIMEOUT",
        "SANDBOX_RUNNER_TIMEOUT",
        "SANDBOX_DENIED",
        "SHELL_ABORTED",
        "TOOL_INPUT_INVALID",
    ] {
        if message.starts_with(&format!("[{code}]")) {
            return ToolBodyError::coded(message, "ShellRuntimeError", code);
        }
    }
    ToolBodyError::coded(message, "ShellRuntimeError", "SHELL_STARTUP_FAILED")
}

fn escalation_mode_for_permissions(
    permissions: Option<&str>,
    justification: Option<&str>,
) -> Result<Option<dsh_sandbox::SandboxMode>, String> {
    if justification.is_some() && permissions.is_none() {
        return Err("`justification` requires an explicit `sandbox_permissions`".into());
    }
    if permissions.is_some() && justification.is_none() {
        return Err("`sandbox_permissions` requires a non-empty `justification`".into());
    }
    match permissions {
        None | Some("use_default") => Ok(None),
        Some("with_additional_permissions") => Ok(Some(dsh_sandbox::SandboxMode::WorkspaceWrite)),
        Some("require_escalated") => Ok(Some(dsh_sandbox::SandboxMode::DangerFullAccess)),
        Some(other) => Err(format!("unsupported sandbox_permissions: {other}")),
    }
}

struct PwshJobHooks {
    process: Arc<dyn ShellProcess>,
    profiles: Option<Arc<dyn dsh_shell::ExecutionProfileResolver>>,
    execution_context_id: Option<String>,
    capability: Option<String>,
}

impl JobHooks for PwshJobHooks {
    fn cancel(&self, _reason: Option<String>) {
        self.process.kill();
    }

    fn done(&self) -> BoxFuture<'static, JobOutcome> {
        let process = self.process.clone();
        let profiles = self.profiles.clone();
        let context = self.execution_context_id.clone();
        let capability = self.capability.clone();
        Box::pin(async move {
            process.done().await;
            let status = match process.status() {
                ShellProcessStatus::Completed if process.exit_code() == Some(0) => {
                    JobOutcomeStatus::Completed
                }
                ShellProcessStatus::Completed => JobOutcomeStatus::Failed,
                ShellProcessStatus::Killed => JobOutcomeStatus::Killed,
                ShellProcessStatus::Running => JobOutcomeStatus::Failed,
            };
            if status != JobOutcomeStatus::Completed {
                if let (Some(profiles), Some(context), Some(capability)) =
                    (profiles, context, capability)
                {
                    profiles.report_failure(&context, &capability);
                }
            }
            JobOutcome {
                status,
                detail: process
                    .signal()
                    .map(|signal| format!("signal: {signal}"))
                    .or_else(|| process.exit_code().map(|code| format!("exit code: {code}"))),
                output: None,
            }
        })
    }

    fn read_output(&self) -> Option<String> {
        let read = self.process.read_output();
        let mut text = read.delta;
        if read.lossy {
            text.push_str("\n[output truncated]");
            if read.stdout_spill_path.is_none() && read.stderr_spill_path.is_none() {
                text.push_str("\n[complete=false; dropped output has no retained complete stream]");
            }
        }
        for (name, path) in [
            ("stdout", read.stdout_spill_path),
            ("stderr", read.stderr_spill_path),
        ] {
            if let Some(path) = path {
                text.push_str(&format!("\n[complete {name} stream: {path}]"));
            }
        }
        Some(text)
    }
}

pub struct ToolPwshService;

fn execution_directory(
    request: &mut ShellExecRequest,
    args: &serde_json::Value,
    owner: Option<&Arc<dyn dsh_agent::Agent>>,
    workspaces: Option<&Arc<dyn dsh_workspace_resources::ManagedWorkspaces>>,
) -> Result<(), ToolBodyError> {
    let Some(policy) = request.sandbox_policy.as_mut() else {
        if let Some(path) = args.get("workdir").and_then(serde_json::Value::as_str) {
            request.workdir = Some(
                std::fs::canonicalize(path)
                    .map_err(|error| {
                        ToolBodyError::plain(format!("Execution directory is unavailable: {error}"))
                    })?
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        return Ok(());
    };
    let project = policy.workspace_root.clone();
    request.dsh_env = Some(vec![("DSH_PROJECT_ROOT".into(), project.clone())]);
    if let Some(path) = args.get("workdir").and_then(serde_json::Value::as_str) {
        let path = std::path::PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            std::path::Path::new(&project).join(path)
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|error| ToolBodyError::plain(format!("执行目录不存在或不可访问：{error}")))?;
        let path = path.to_string_lossy().into_owned();
        if let (Some(owner), Some(workspaces)) = (owner, workspaces) {
            if let Some(copy) = workspaces
                .resolve(owner.id().as_str(), &path)
                .map_err(ToolBodyError::plain)?
            {
                policy.workspace_root = copy.root;
                policy.read_only_roots = copy.read_only_roots;
                request
                    .dsh_env
                    .as_mut()
                    .unwrap()
                    .push(("GIT_OPTIONAL_LOCKS".into(), "0".into()));
            }
        }
        request.workdir = Some(path);
    }
    Ok(())
}

fn outside_execution_directory(request: &ShellExecRequest) -> Option<String> {
    let policy = request.sandbox_policy.as_ref()?;
    if policy.mode == dsh_sandbox::SandboxMode::DangerFullAccess {
        return None;
    }
    let directory = std::fs::canonicalize(request.workdir.as_ref()?).ok()?;
    let roots = std::iter::once(policy.workspace_root.clone())
        .chain(policy.read_only_roots.clone())
        .chain(dsh_sandbox::writable_roots(policy));
    for root in roots {
        if std::fs::canonicalize(root).is_ok_and(|root| directory.starts_with(root)) {
            return None;
        }
    }
    Some(directory.to_string_lossy().into_owned())
}

async fn authorize_execution_directory(
    request: &mut ShellExecRequest,
    owner: Option<&Arc<dyn dsh_agent::Agent>>,
    approval: Option<&Arc<ApprovalService>>,
    call_id: &str,
) -> Result<(), ToolBodyError> {
    let Some(directory) = outside_execution_directory(request) else {
        return Ok(());
    };
    let agent = owner.cloned().ok_or_else(|| {
        ToolBodyError::coded(
            "Cross-directory execution requires an initiating agent",
            "ApprovalError",
            "APPROVAL_UNAVAILABLE",
        )
    })?;
    let approval = approval.ok_or_else(|| {
        ToolBodyError::coded(
            "Cross-directory execution requires an approval service",
            "ApprovalError",
            "APPROVAL_UNAVAILABLE",
        )
    })?;
    let mode = request.sandbox_policy.as_ref().unwrap().mode;
    let outcome = approval.request(&ApprovalRequest {
        agent, tool_name: "pwsh".into(), call_id: Some(call_id.into()),
        reason: Some(format!("本次命令需要在工作区外目录执行：{directory}。仅为此命令授予该目录的 {} 权限，当前会话工作区保持不变。", mode.as_str())),
        grant_key: None, rememberable: false, signal: request.signal.clone(),
    }).await.map_err(ToolBodyError::plain)?;
    if !matches!(
        outcome,
        ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways
    ) {
        return Err(ToolBodyError::coded(
            format!(
                "Cross-directory execution was not approved: {}. The command did not run.",
                outcome.as_str()
            ),
            "ApprovalError",
            "APPROVAL_REJECTED",
        ));
    }
    let policy = request.sandbox_policy.as_mut().unwrap();
    let project = std::mem::replace(&mut policy.workspace_root, directory);
    if !policy.read_only_roots.contains(&project) {
        policy.read_only_roots.push(project);
    }
    Ok(())
}

pub fn removes_directory(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("[system.io.directory]::delete")
        || normalized.contains("[io.directory]::delete")
        || normalized
            .split(|character: char| {
                character.is_whitespace() || character == ';' || character == '|'
            })
            .any(|token| matches!(token, "remove-item" | "ri" | "rm" | "rmdir" | "rd"))
}

fn execution_removes_directory(name: &str, arguments: &serde_json::Value) -> bool {
    let native = |arguments: &serde_json::Value| {
        let program = arguments["program"].as_str().unwrap_or_default();
        let base = std::path::Path::new(program)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(program);
        matches!(
            base.to_ascii_lowercase().as_str(),
            "rm" | "rmdir" | "rd" | "del"
        ) || arguments["argv"].as_array().is_some_and(|items| {
            items
                .iter()
                .filter_map(|value| value.as_str())
                .any(removes_directory)
        })
    };
    match name {
        "pwsh" => removes_directory(arguments["command"].as_str().unwrap_or_default()),
        "execute_script" => removes_directory(arguments["script"].as_str().unwrap_or_default()),
        "execute_native" => native(arguments),
        "execute_steps" => arguments["steps"]
            .as_array()
            .is_some_and(|steps| steps.iter().any(native)),
        _ => false,
    }
}

fn apply_profile(
    request: &mut ShellExecRequest,
    profiles: Option<&Arc<dyn dsh_shell::ExecutionProfileResolver>>,
    powershell: bool,
) -> Result<Option<dsh_shell::ResolvedExecutionProfile>, ToolBodyError> {
    let Some(profiles) = profiles else {
        return Ok(None);
    };
    let cwd = request
        .workdir
        .clone()
        .or_else(|| {
            request
                .sandbox_policy
                .as_ref()
                .map(|policy| policy.workspace_root.clone())
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let profile = profiles
        .resolve(
            request
                .sandbox_policy
                .as_ref()
                .and_then(|policy| policy.session_id.as_ref())
                .map(|id| id.as_str()),
            &cwd,
        )
        .map_err(|message| {
            ToolBodyError::coded(message, "EnvironmentError", "ENVIRONMENT_UNAVAILABLE")
        })?;
    if powershell
        && !matches!(
            profile.shell_kind.as_str(),
            "pwsh" | "powershell" | "powershell5" | "powershell7" | "ps5" | "ps7" | "auto" | ""
        )
    {
        return Err(ToolBodyError::coded(
            "The selected shell does not accept PowerShell syntax. Use execute_native with the selected shell executable and its explicit argv, or select PowerShell.",
            "EnvironmentError",
            "SHELL_LANGUAGE_MISMATCH",
        ));
    }
    request.shell_path = profile.shell_path.clone();
    request.execution_context_id = Some(profile.context_id.clone());
    Ok(Some(profile))
}

async fn validate_profile(
    request: &ShellExecRequest,
    profiles: Option<&Arc<dyn dsh_shell::ExecutionProfileResolver>>,
    capability: &str,
    executable: Option<String>,
    shell_kind: Option<String>,
) -> Result<(), ToolBodyError> {
    let Some(profiles) = profiles else {
        return Ok(());
    };
    profiles
        .validate(dsh_shell::ExecutionValidationRequest {
            session_id: request
                .sandbox_policy
                .as_ref()
                .and_then(|policy| policy.session_id.as_ref())
                .map(|id| id.as_str().into()),
            workdir: request
                .workdir
                .clone()
                .or_else(|| {
                    request
                        .sandbox_policy
                        .as_ref()
                        .map(|policy| policy.workspace_root.clone())
                })
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            capability: capability.into(),
            executable,
            shell_kind,
            execution_context_id: request.execution_context_id.clone(),
            sandbox_policy: request.sandbox_policy.clone(),
            signal: request.signal.clone(),
        })
        .await
        .map_err(|error| {
            ToolBodyError::coded(error, "EnvironmentError", "ENVIRONMENT_VALIDATION_FAILED")
        })
}

impl ToolPwshService {
    pub fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-pwsh requires the tools service".to_string())?;
        let shell = ctx
            .get_typed::<Arc<dyn ShellExecutor>>("shell", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-pwsh requires the shell service".to_string())?;
        let jobs = ctx
            .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "tool-pwsh requires the jobs service".to_string())?;
        let sandbox_policy = ctx
            .get_typed::<Arc<SandboxPolicyService>>("sandboxPolicy", false)
            .map(|slot| slot.as_ref().clone());
        let approval = ctx
            .get_typed::<Arc<ApprovalService>>("approval", false)
            .map(|slot| slot.as_ref().clone());

        let approval_listener: Arc<Listener> = Arc::new(|_ctx, args| {
            let execution = args
                .first()
                .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
                .map(|slot| slot.as_ref().clone());
            let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
            Box::pin(async move {
                if let Some(execution) = execution
                    && execution_removes_directory(&execution.name, &execution.arguments)
                {
                    return Some(arc(PreToolDecision::Ask {
                        reason: Some("删除文件夹需要用户确认".to_string()),
                        grant_key: Some(format!(
                            "shell:{}:{}",
                            execution.name, execution.arguments,
                        )),
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
            approval_listener,
            EventOptions::default().global(true),
        ));

        native::install(
            ctx,
            tools.clone(),
            shell.clone(),
            jobs.clone(),
            sandbox_policy.clone(),
            approval.clone(),
        )?;
        let execute_profiles = ctx
            .get_typed::<Arc<dyn dsh_shell::ExecutionProfileResolver>>("executionProfiles", false)
            .map(|slot| slot.as_ref().clone());
        let execute_sandbox = ctx
            .get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox", false)
            .map(|slot| slot.as_ref().clone());
        let execute_shell = shell.clone();
        let execute_jobs = jobs.clone();
        let execute_policy = sandbox_policy.clone();
        let execute_approval = approval.clone();
        let execute_workspaces = ctx
            .get_typed::<Arc<dyn dsh_workspace_resources::ManagedWorkspaces>>(
                "managedWorkspaces",
                false,
            )
            .map(|slot| slot.as_ref().clone());
        tools.register(
            ctx,
            ToolDefinition {
                name: "pwsh".to_string(),
                description:
                    "Execute PowerShell using the selected PowerShell environment. Arbitrary compound scripts do not guarantee that every failed native step stops later writes; use execute_native with argv for literal arguments and execute_steps for dependent operations. Native stderr is diagnostic text; use execute_native to preserve its exit status without PowerShell 5 redirection semantics. allow_nonzero returns diagnostic data, never proves business success or resumes an interrupted script. Timeout and cancellation do not roll back effects."
                        .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                            "command": { "type": "string" },
                            "workdir": { "type": "string", "description": "Working directory. An external directory automatically requests one-command scoped approval before launch; the session workspace is unchanged. Managed copies preserve the source project as read-only." },
                            "description": { "type": "string" },
                            "sandbox_permissions": { "type": "string", "enum": ["use_default", "with_additional_permissions", "require_escalated"], "description": "Request a wider sandbox for this exact command; requires justification and user approval." },
                            "justification": { "type": "string", "description": "Why this command needs the requested wider sandbox." },
                        "timeout_ms": { "type": "integer", "description": "Foreground execution budget in milliseconds, from 1 to 600000; use a short budget for simple probes and background jobs for long work." },
                        "run_in_background": { "type": "boolean" },
                        "allow_nonzero": { "type": "boolean", "description": "Return read-only diagnostic output even when a probe exits nonzero; the exitCode remains visible." }
                    },
                    "required": ["command", "description"]
                }),
                output: ToolOutputDefinition {
                    schema: output::schema(),
                    render: Arc::new(|_args, value| {
                        let text = if value["kind"] == "background" {
                            format!(
                                "started background job {}",
                                value["jobId"].as_str().unwrap_or_default()
                            )
                        } else {
                            output::render_result(value)
                        };
                        Ok(vec![ContentBlock::Text { text }])
                    }),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(move |args, run: &ToolRunContext| {
                    let shell = execute_shell.clone();
                    let jobs = execute_jobs.clone();
                    let sandbox_policy = execute_policy.clone();
                    let approval = execute_approval.clone();
                    let workspaces=execute_workspaces.clone();
                    let profiles = execute_profiles.clone();
                    let sandbox = execute_sandbox.clone();
                    let args = args.clone();
                    let signal = run.execution.signal.lock().clone();
                    let owner = run.execution.agent.clone();
                    let call_id = run.execution.call_id.to_string();
                    let mark_effects=run.track_requested_effects();
                    Box::pin(async move {
                        let command = args
                            .get("command")
                            .and_then(|value| value.as_str())
                            .filter(|command| !command.trim().is_empty())
                            .ok_or_else(|| {
                                ToolBodyError::plain("invalid command: expected a non-empty string")
                            })?;
                        let timeout_ms = match args.get("timeout_ms") {
                            None => None,
                            Some(value) => Some(value.as_u64().filter(|value| (1..=600_000).contains(value))
                                .ok_or_else(|| ToolBodyError::coded("timeout_ms must be an integer from 1 to 600000", "ToolInputError", "TOOL_INPUT_INVALID"))?),
                        };
                        let requested_permissions = args
                            .get("sandbox_permissions")
                            .and_then(serde_json::Value::as_str)
                            .filter(|value| *value != "use_default");
                        let justification = args
                            .get("justification")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        if justification.is_some() && requested_permissions.is_none() {
                            return Err(ToolBodyError::coded(
                                "`justification` requires an explicit `sandbox_permissions`",
                                "ToolInputError",
                                "TOOL_INPUT_INVALID",
                            ));
                        }
                        let escalation_mode = escalation_mode_for_permissions(
                            requested_permissions,
                            justification,
                        )
                        .map_err(|message| {
                            ToolBodyError::coded(message, "ToolInputError", "TOOL_INPUT_INVALID")
                        })?;
                        if let Some(mode) = escalation_mode {
                            let agent = owner.clone().ok_or_else(|| {
                                ToolBodyError::plain(
                                    "sandbox escalation requires an initiating agent",
                                )
                            })?;
                            let approval = approval.clone().ok_or_else(|| {
                                ToolBodyError::coded(
                                    "sandbox escalation requires an approval service",
                                    "ApprovalError",
                                    "APPROVAL_UNAVAILABLE",
                                )
                            })?;
                            let outcome = approval
                                .request(&ApprovalRequest {
                                    agent,
                                    tool_name: "pwsh".into(),
                                    call_id: Some(call_id.clone()),
                                    reason: Some(format!(
                                        "PowerShell requests sandbox mode {}: {}",
                                        mode.as_str(),
                                        justification.unwrap_or("explicit model request"),
                                    )),
                                    grant_key: None,
                                    rememberable: false,
                                    signal: Some(signal.clone()),
                                })
                                .await
                                .map_err(ToolBodyError::plain)?;
                            if !matches!(
                                outcome,
                                ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways
                            ) {
                                return Err(ToolBodyError::coded(
                                    format!("sandbox escalation was not approved: {}", outcome.as_str()),
                                    "ApprovalError",
                                    "APPROVAL_REJECTED",
                                ));
                            }
                        }
                        if args.get("run_in_background") == Some(&serde_json::Value::Bool(true)) {
                            let description = args
                                .get("description")
                                .and_then(|value| value.as_str())
                                .filter(|description| !description.trim().is_empty())
                                .ok_or_else(|| {
                                    ToolBodyError::plain(
                                        "invalid description: expected a non-empty string",
                                    )
                                })?
                                .to_string();
                            let mut request = ShellExecRequest::new(command);
                            request.signal = Some(signal);
                            if let Some(owner) = owner.as_ref() {
                                let policy = sandbox_policy
                                    .as_ref()
                                    .ok_or_else(|| {
                                        ToolBodyError::plain(
                                            "tool-pwsh requires sandboxPolicy for agent calls",
                                        )
                                    })?
                                    .resolve(&SandboxPolicyRequest {
                                        session: Some(Arc::new(owner.session().clone())),
                                        mode: escalation_mode,
                                    });
                                request.sandbox_policy = Some(policy);
                            }
                            execution_directory(&mut request,&args,owner.as_ref(),workspaces.as_ref())?;
                            apply_profile(&mut request, profiles.as_ref(), true)?;
                            authorize_execution_directory(&mut request,owner.as_ref(),approval.as_ref(),&call_id).await?;
                            validate_profile(&request, profiles.as_ref(), "shell", request.shell_path.clone(), Some("powershell".into())).await?;
                            if let (Some(sandbox), Some(policy)) = (&sandbox, &request.sandbox_policy) {
                                sandbox.prepare(policy).await.map_err(shell_runtime_failure)?;
                            }
                            let spec = shell.resolve(request);
                            mark_effects();
                            let process_shell = shell.clone();
                            let id = jobs
                                .start(JobStart {
                                    kind: "pwsh".to_string(),
                                    label: description,
                                    output_limit_bytes: None,
                                    owner,
                                    run: Arc::new(move || {
                                        Arc::new(PwshJobHooks {
                                            process: process_shell.start(spec.clone()),
                                            profiles: profiles.clone(),
                                            execution_context_id: spec.execution_context_id.clone(),
                                            capability: Some("shell".into()),
                                        })
                                    }),
                                })
                                .map_err(ToolBodyError::plain)?;
                            return Ok(serde_json::json!({
                                "kind": "background",
                                "jobId": id.as_str(),
                            }));
                        }
                        let mut request = ShellExecRequest::new(command);
                        request.timeout_ms = timeout_ms;
                        request.signal = Some(signal);
                        if let Some(owner) = owner.as_ref() {
                            let policy = sandbox_policy
                                .as_ref()
                                .ok_or_else(|| {
                                    ToolBodyError::plain(
                                        "tool-pwsh requires sandboxPolicy for agent calls",
                                    )
                                })?
                                .resolve(&SandboxPolicyRequest {
                                    session: Some(Arc::new(owner.session().clone())),
                                    mode: escalation_mode,
                                });
                            request.sandbox_policy = Some(policy);
                        }
                        execution_directory(&mut request,&args,owner.as_ref(),workspaces.as_ref())?;
                        apply_profile(&mut request, profiles.as_ref(), true)?;
                        authorize_execution_directory(&mut request,owner.as_ref(),approval.as_ref(),&call_id).await?;
                        validate_profile(&request, profiles.as_ref(), "shell", request.shell_path.clone(), Some("powershell".into())).await?;
                        let retry_context = serde_json::json!({"command":request.command,"workdir":request.workdir,"context":request.execution_context_id,"policy":request.sandbox_policy.as_ref().map(|p|format!("{:?}",p))});
                        let context_id = request.execution_context_id.clone();
                        if let (Some(sandbox),Some(policy))=(&sandbox,&request.sandbox_policy){sandbox.prepare(policy).await.map_err(shell_runtime_failure)?;}
                        mark_effects();
                        let result = shell
                            .run(shell.resolve(request))
                            .await
                            .map_err(|error| {
                                if let (Some(profiles),Some(context)) = (&profiles,&context_id) { profiles.report_failure(context,"shell"); }
                                shell_runtime_failure(error)
                            })?;
                        let allow_nonzero = args.get("allow_nonzero") == Some(&serde_json::Value::Bool(true));
                        if result.exit_code != Some(0) || result.signal.is_some() || result.timed_out || result.aborted {
                            if let (Some(profiles),Some(context)) = (&profiles,&result.execution_context_id) { profiles.report_failure(context,"shell"); }
                        }
                        let mut receipt = output::result_json(&result, &call_id);
                        receipt["retryContext"] = retry_context;
                        let output = output::render_result(&receipt);
                        if result.aborted {
                            return Err(ToolBodyError::coded(format!("PowerShell command cancelled\n{output}"), "AbortError", "SHELL_ABORTED"));
                        }
                        if result.timed_out {
                            return Err(ToolBodyError::coded(format!("PowerShell command timed out after {} ms\n{output}", result.timeout_ms), "ShellError", "SHELL_TIMEOUT"));
                        }
                        if let Some(sandbox) = &result.sandbox {
                            if sandbox.runner_failed == Some(true) {
                                return Err(ToolBodyError::coded(format!("Sandbox startup or cleanup failed; inspect the runtime before retrying the command.\n{output}"), "SandboxError", "SANDBOX_RUNNER_FAILED"));
                            }
                            if sandbox.denied {
                                return Err(ToolBodyError::coded(format!("The execution policy denied an operation. Diagnose the specific target and authorized scope before requesting any additional permission.\n{output}"), "SandboxError", "SANDBOX_DENIED"));
                            }
                        }
                        if (result.exit_code != Some(0) || result.signal.is_some()) && !allow_nonzero {
                            let hint = if output.contains("fatal: not a git repository") {
                                "\nThis directory is not a Git repository. For optional inspection, first use Invoke-DshNativeProbe -FilePath git -ArgumentList @('rev-parse','--is-inside-work-tree'); run Git status/log only if ExitCode is 0. Continue ordinary file inspection without Git."
                            } else if output.contains("Unable to read current working directory") {
                                "\nThe current directory is unreadable; this is not evidence that .git is missing. Check the resolved workdir and sandbox runtime permissions before retrying."
                            } else if output.contains("NativeCommandError") || output.contains("NativeCommandExitException") {
                                "\nPowerShell interrupted native stderr handling; stderr text alone is not a failing exit code. For native tests or dependency checks with redirected stderr, use Invoke-DshNativeProbe and inspect its ExitCode and complete Output; allow_nonzero alone cannot resume an interrupted script."
                            } else if result.sandbox.as_ref().is_some_and(|s| s.mode != dsh_sandbox::SandboxMode::DangerFullAccess)
                                && (output.contains("FileNotFoundError") || output.contains("不存在") || output.contains("No such file")) {
                                "\nVerify the exact path and chosen execution context before diagnosing a missing file or encoding issue. This message alone does not establish a sandbox denial. Diagnose the required target and inspect existing effects before any retry."
                            } else { "" };
                            return Err(ToolBodyError::coded(format!("PowerShell command failed (exit: {:?}, signal: {:?})\n{output}{hint}", result.exit_code, result.signal), "ShellError", "SHELL_FAILED").with_receipt(receipt));
                        }
                        Ok(receipt)
                    })
                }),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )?;
        Ok(Arc::new(Self))
    }
}

#[cfg(test)]
mod tests {
    use super::escalation_mode_for_permissions;

    #[tokio::test]
    async fn external_workdir_is_gated_and_missing_approval_fails_closed() {
        use super::*;
        let root = std::env::temp_dir().join(format!("dsh-workdir-gate-{}", std::process::id()));
        let project = root.join("project");
        let external = root.join("external");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let mut request = ShellExecRequest::new("Get-ChildItem");
        request.workdir = Some(external.to_string_lossy().into_owned());
        request.sandbox_policy = Some(dsh_sandbox::SandboxExecutionPolicy {
            mode: dsh_sandbox::SandboxMode::ReadOnly,
            workspace_root: project.to_string_lossy().into_owned(),
            read_only_roots: vec![],
            session_id: None,
        });
        let before = request.sandbox_policy.clone();
        assert!(outside_execution_directory(&request).is_some());
        assert!(
            authorize_execution_directory(&mut request, None, None, "probe")
                .await
                .is_err()
        );
        assert_eq!(
            request.sandbox_policy, before,
            "failure cannot mutate the execution policy"
        );
        request
            .sandbox_policy
            .as_mut()
            .unwrap()
            .read_only_roots
            .push(external.to_string_lossy().into_owned());
        assert!(
            outside_execution_directory(&request).is_none(),
            "already granted read roots need no new approval"
        );
        request
            .sandbox_policy
            .as_mut()
            .unwrap()
            .read_only_roots
            .clear();
        request.workdir = Some(project.to_string_lossy().into_owned());
        assert!(outside_execution_directory(&request).is_none());
        std::fs::remove_dir(&external).unwrap();
        std::fs::remove_dir(&project).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn explicit_escalation_modes_are_fail_closed_and_justified() {
        assert!(
            escalation_mode_for_permissions(None, None)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            escalation_mode_for_permissions(
                Some("with_additional_permissions"),
                Some("read tests")
            )
            .unwrap()
            .unwrap()
            .as_str(),
            "workspace-write"
        );
        assert_eq!(
            escalation_mode_for_permissions(Some("require_escalated"), Some("run external test"))
                .unwrap()
                .unwrap()
                .as_str(),
            "danger-full-access"
        );
        assert!(escalation_mode_for_permissions(None, Some("missing mode")).is_err());
        assert!(escalation_mode_for_permissions(Some("require_escalated"), None).is_err());
        assert!(escalation_mode_for_permissions(Some("unknown"), Some("bad mode")).is_err());
    }
}
