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
use futures::future::BoxFuture;

struct PwshJobHooks {
    process: Arc<dyn ShellProcess>,
}

impl JobHooks for PwshJobHooks {
    fn cancel(&self, _reason: Option<String>) {
        self.process.kill();
    }

    fn done(&self) -> BoxFuture<'static, JobOutcome> {
        let process = self.process.clone();
        Box::pin(async move {
            process.done().await;
            let status = match process.status() {
                ShellProcessStatus::Completed => JobOutcomeStatus::Completed,
                ShellProcessStatus::Killed => JobOutcomeStatus::Killed,
                ShellProcessStatus::Running => JobOutcomeStatus::Failed,
            };
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
        }
        Some(text)
    }
}

pub struct ToolPwshService;

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

        let approval_listener: Arc<Listener> = Arc::new(|_ctx, args| {
            let execution = args
                .first()
                .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
                .map(|slot| slot.as_ref().clone());
            let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
            Box::pin(async move {
                if let Some(execution) = execution
                    && execution.name == "pwsh"
                    && removes_directory(
                        execution
                            .arguments
                            .get("command")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default(),
                    )
                {
                    return Some(arc(PreToolDecision::Ask {
                        reason: Some("删除文件夹需要用户确认".to_string()),
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

        let execute_shell = shell.clone();
        let execute_jobs = jobs.clone();
        let execute_policy = sandbox_policy.clone();
        tools.register(
            ctx,
            ToolDefinition {
                name: "pwsh".to_string(),
                description:
                    "Execute a PowerShell command in the foreground or as a background job."
                        .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "command": { "type": "string" },
                        "description": { "type": "string" },
                        "run_in_background": { "type": "boolean" }
                    },
                    "required": ["command", "description"]
                }),
                output: ToolOutputDefinition {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "enum": ["foreground", "background"] },
                            "jobId": { "type": "string" },
                            "exitCode": { "oneOf": [{ "type": "integer" }, { "type": "null" }] },
                            "stdout": { "type": "string" }
                        },
                        "required": ["kind"]
                    }),
                    render: Arc::new(|_args, value| {
                        let text = if value["kind"] == "background" {
                            format!(
                                "started background job {}",
                                value["jobId"].as_str().unwrap_or_default()
                            )
                        } else {
                            value["stdout"].as_str().unwrap_or_default().to_string()
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
                    let args = args.clone();
                    let signal = run.execution.signal.lock().clone();
                    let owner = run.execution.agent.clone();
                    Box::pin(async move {
                        let command = args
                            .get("command")
                            .and_then(|value| value.as_str())
                            .filter(|command| !command.trim().is_empty())
                            .ok_or_else(|| {
                                ToolBodyError::plain("invalid command: expected a non-empty string")
                            })?;
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
                                        mode: None,
                                    });
                                request.sandbox_policy = Some(policy);
                            }
                            let spec = shell.resolve(request);
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
                                    mode: None,
                                });
                            request.sandbox_policy = Some(policy);
                        }
                        let result = shell
                            .run(shell.resolve(request))
                            .await
                            .map_err(ToolBodyError::plain)?;
                        Ok(serde_json::json!({
                            "kind": "foreground",
                            "exitCode": result.exit_code,
                            "stdout": result.stdout.text,
                        }))
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
