//! Literal argv execution and sequential dependency boundaries. All dispatches
//! use the ordinary shell executor's subprocess and sandbox lifecycle.

use std::sync::Arc;

use cordis::Context;
use dsh_jobs::{JobRegistry, JobStart};
use dsh_llm::ContentBlock;
use dsh_sandbox_policy::{SandboxPolicyRequest, SandboxPolicyService};
use dsh_shell::{ShellExecRequest, ShellExecutor};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalService};
use serde_json::{Value, json};

fn input_error(message: impl Into<String>) -> ToolBodyError {
    ToolBodyError::coded(message, "ToolInputError", "TOOL_INPUT_INVALID")
}

fn argv(value: &Value) -> Result<Vec<String>, ToolBodyError> {
    let program = value["program"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            input_error(
                "program must be a non-empty native executable name, path or @profile reference",
            )
        })?;
    let values = value["argv"]
        .as_array()
        .ok_or_else(|| input_error("argv must be an array of strings"))?;
    if values.len() > 4096 {
        return Err(input_error("argv exceeds 4096 arguments"));
    }
    let mut arguments = vec![program.to_string()];
    for value in values {
        arguments.push(
            value
                .as_str()
                .ok_or_else(|| input_error("every argv item must be a string"))?
                .to_string(),
        );
    }
    dsh_shell::validate_native_argv(&arguments).map_err(input_error)?;
    Ok(arguments)
}

fn resolve_program(
    arguments: &mut [String],
    profile: Option<&dsh_shell::ResolvedExecutionProfile>,
) -> Result<(), ToolBodyError> {
    let explicit_reference = arguments[0].starts_with('@');
    let reference = arguments[0].strip_prefix('@').unwrap_or(&arguments[0]);
    let reference = match reference {
        "python3" | "python.exe" | "python3.exe" => "python",
        "cargo.exe" => "cargo",
        "rustc.exe" => "rustc",
        "git.exe" => "git",
        "node.exe" => "node",
        "rg.exe" => "rg",
        "ffmpeg.exe" => "ffmpeg",
        other => other,
    };
    if !explicit_reference
        && (profile.is_none()
            || (reference != "python" && !profile.unwrap().toolchain_paths.contains_key(reference)))
    {
        return Ok(());
    }
    let profile = profile
        .ok_or_else(|| input_error("profile references require the executionProfiles service"))?;
    let program = match reference {
        "python" => profile.python_path.as_ref(),
        "shell" => profile.shell_path.as_ref(),
        name => profile.toolchain_paths.get(name),
    }
    .filter(|value| !value.is_empty())
    .ok_or_else(|| {
        ToolBodyError::coded(
            format!("Selected environment has no resolved program for @{reference}"),
            "EnvironmentError",
            "ENVIRONMENT_UNAVAILABLE",
        )
    })?;
    arguments[0] = program.clone();
    dsh_shell::validate_native_argv(arguments).map_err(input_error)
}

fn script_argv(
    args: &Value,
    profile: Option<&dsh_shell::ResolvedExecutionProfile>,
) -> Result<Vec<String>, ToolBodyError> {
    let script = args["script"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| input_error("script must be non-empty"))?;
    let language = args["language"]
        .as_str()
        .ok_or_else(|| input_error("language is required"))?;
    let profile = profile.ok_or_else(|| {
        ToolBodyError::coded(
            "Script execution requires a selected execution profile",
            "EnvironmentError",
            "ENVIRONMENT_UNAVAILABLE",
        )
    })?;
    let kind = match profile.shell_kind.as_str() {
        "pwsh" | "powershell" | "powershell5" | "powershell7" | "ps5" | "ps7" => "powershell",
        other => other,
    };
    if kind != language {
        return Err(ToolBodyError::coded(
            format!(
                "Selected shell uses {kind}; requested script language is {language}. Select the matching environment before execution."
            ),
            "EnvironmentError",
            "SHELL_LANGUAGE_MISMATCH",
        ));
    }
    let executable = profile
        .shell_path
        .clone()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            ToolBodyError::coded(
                "Selected shell has no resolved executable",
                "EnvironmentError",
                "ENVIRONMENT_UNAVAILABLE",
            )
        })?;
    let mut argv = vec![executable];
    match language {
        "bash" => argv.extend(["--noprofile".into(),"--norc".into(),"-c".into(),script.into()]),
        "zsh" => argv.extend(["-f".into(),"-c".into(),script.into()]),
        "powershell" => argv.extend(["-NoLogo".into(),"-NoProfile".into(),"-NonInteractive".into(),"-Command".into(),format!("$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $OutputEncoding=[System.Text.Encoding]::UTF8; $PSNativeCommandUseErrorActionPreference=$true; $global:LASTEXITCODE=0;\n{script}\nif ($global:LASTEXITCODE -ne 0) {{ exit $global:LASTEXITCODE }}")]),
        _ => return Err(input_error("language must be powershell, bash or zsh")),
    }
    dsh_shell::validate_native_argv(&argv).map_err(input_error)?;
    Ok(argv)
}

pub(super) fn install(
    ctx: &Context,
    tools: Arc<ToolRuntime>,
    shell: Arc<dyn ShellExecutor>,
    jobs: Arc<dyn JobRegistry>,
    policy: Option<Arc<SandboxPolicyService>>,
    approval: Option<Arc<ApprovalService>>,
) -> Result<(), String> {
    let profiles = ctx
        .get_typed::<Arc<dyn dsh_shell::ExecutionProfileResolver>>("executionProfiles", false)
        .map(|slot| slot.as_ref().clone());
    let workspaces = ctx
        .get_typed::<Arc<dyn dsh_workspace_resources::ManagedWorkspaces>>(
            "managedWorkspaces",
            false,
        )
        .map(|slot| slot.as_ref().clone());
    let sandbox = ctx
        .get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox", false)
        .map(|slot| slot.as_ref().clone());
    for name in ["execute_native", "execute_steps", "execute_script"] {
        let sequence = name == "execute_steps";
        let script = name == "execute_script";
        let mut properties = json!({
            "description":{"type":"string"},
            "workdir":{"type":"string"},
            "timeout_ms":{"type":"integer", "minimum":1, "maximum":600000},
            "allow_nonzero":{"type":"boolean", "description":"Return a diagnostic result on nonzero exit; never runs dependent steps after failure."},
            "sandbox_permissions":{"type":"string", "enum":["use_default","with_additional_permissions","require_escalated"]},
            "justification":{"type":"string"}
        });
        let required;
        if script {
            properties["script"] = json!({"type":"string"});
            properties["language"] = json!({"type":"string","enum":["powershell","bash","zsh"]});
            properties["run_in_background"] = json!({"type":"boolean"});
            required = json!(["script", "language", "description"]);
        } else if sequence {
            properties["steps"] = json!({"type":"array", "minItems":1, "maxItems":32, "items":{
                "type":"object", "additionalProperties":false,
                "properties":{"program":{"type":"string"},"argv":{"type":"array","items":{"type":"string"}}},
                "required":["program","argv"]
            }});
            required = json!(["steps", "description"]);
        } else {
            properties["program"] = json!({"type":"string", "description":"Native executable path/name, @python, @shell or configured @toolchain name."});
            properties["argv"] = json!({"type":"array", "items":{"type":"string"}});
            properties["run_in_background"] = json!({"type":"boolean"});
            required = json!(["program", "argv", "description"]);
        }
        let (shell, jobs, policy, approval, profiles, workspaces, sandbox) = (
            shell.clone(),
            jobs.clone(),
            policy.clone(),
            approval.clone(),
            profiles.clone(),
            workspaces.clone(),
            sandbox.clone(),
        );
        tools.register(ctx, ToolDefinition {
            name: name.into(),
            description: if script {
                "Execute a script using the explicitly selected matching PowerShell, Bash or Zsh environment under the current sandbox. State its language; a mismatch fails before launch. Final shell status does not prove every subcommand succeeded and scripts are not transactions. Use execute_steps for checked dependencies; use execute_native for literal program argv. For missing tools or access failures use environment_probe then environment_validate; do not guess installation paths or rotate shell wrappers."
            } else if sequence {
                "Execute dependent native programs sequentially with literal argv. The first nonzero exit, signal, cancellation, timeout or startup failure stops dispatch of every remaining step. Preserve each executed step's output and status. Earlier effects are not rolled back. Use scripts as files and native interpreters for complex programs."
            } else {
                "Execute one native program with a literal argv array, preserving spaces, quotes, empty arguments and Unicode without PowerShell parsing. Uses the current sandbox and selected environment; choosing an executable does not grant access. Batch files and shell builtins need an explicit shell adapter. Exit 0 proves only process success; validate the resulting artifact separately."
            }.into(),
            parameters: json!({"type":"object","additionalProperties":false,"properties":properties,"required":required}),
            output: ToolOutputDefinition {
                schema: super::output::schema(),
                render: Arc::new(|_, value| Ok(vec![ContentBlock::Text { text: if value["kind"] == "background" {
                    format!("started background job {}; completion: running", value["jobId"].as_str().unwrap_or_default())
                } else { super::output::render_result(value) } }])),
                presentation_meta:None,
            },
            timeout_ms:None, is_concurrency_safe:None,
            execute:Arc::new(move |args, run| {
                let (shell, jobs, policy, approval, profiles, workspaces, sandbox) = (shell.clone(),jobs.clone(),policy.clone(),approval.clone(),profiles.clone(),workspaces.clone(),sandbox.clone());
                let args = args.clone();
                let owner = run.execution.agent.clone();
                let signal = run.execution.signal.lock().clone();
                let call_id = run.execution.call_id.to_string();
                let mark_effects=run.track_requested_effects();
                Box::pin(async move {
                    let mut commands = if script {
                        Vec::new()
                    } else if sequence {
                        let items = args["steps"].as_array().filter(|items| !items.is_empty() && items.len() <= 32).ok_or_else(|| input_error("steps must contain 1 to 32 native invocations"))?;
                        items.iter().map(argv).collect::<Result<Vec<_>, _>>()?
                    } else { vec![argv(&args)?] };
                    let timeout = match args.get("timeout_ms") { None=>None, Some(value)=>Some(value.as_u64().filter(|value| (1..=600_000).contains(value)).ok_or_else(|| input_error("timeout_ms must be from 1 to 600000"))?) };
                    let permissions = args["sandbox_permissions"].as_str().filter(|value| *value != "use_default");
                    let justification = args["justification"].as_str().map(str::trim).filter(|value| !value.is_empty());
                    let escalation = super::escalation_mode_for_permissions(permissions, justification).map_err(input_error)?;
                    if let Some(mode) = escalation {
                        let owner = owner.clone().ok_or_else(|| input_error("sandbox changes require an initiating agent"))?;
                        let service = approval.as_ref().ok_or_else(|| ToolBodyError::coded("Approval service unavailable", "ApprovalError", "APPROVAL_UNAVAILABLE"))?;
                        let outcome = service.request(&ApprovalRequest { agent:owner, tool_name:name.into(), call_id:Some(call_id.clone()), reason:Some(format!("Native execution requests {}: {}", mode.as_str(), justification.unwrap_or_default())), grant_key:None, rememberable:false, signal:Some(signal.clone()) }).await.map_err(ToolBodyError::plain)?;
                        if !matches!(outcome, ApprovalOutcome::AllowedOnce | ApprovalOutcome::AllowedAlways) {
                            return Err(ToolBodyError::coded("Sandbox change was not approved; no command started", "ApprovalError", "APPROVAL_REJECTED"));
                        }
                    }
                    let mut request = ShellExecRequest::new("");
                    request.signal = Some(signal.clone());
                    request.timeout_ms = timeout;
                    if let Some(owner) = &owner {
                        request.sandbox_policy = Some(policy.as_ref().ok_or_else(|| ToolBodyError::plain("Native execution requires sandboxPolicy for agent calls"))?.resolve(&SandboxPolicyRequest { session:Some(Arc::new(owner.session().clone())), mode:escalation }));
                    }
                    super::execution_directory(&mut request, &args, owner.as_ref(), workspaces.as_ref())?;
                    let profile = super::apply_profile(&mut request, profiles.as_ref(), false)?;
                    if script { commands.push(script_argv(&args, profile.as_ref())?); }
                    for command in &mut commands { resolve_program(command, profile.as_ref())?; }
                    super::authorize_execution_directory(&mut request, owner.as_ref(), approval.as_ref(), &call_id).await?;
                    let capabilities = commands.iter().map(|command| {
                        profile.as_ref().and_then(|profile| {
                            if profile.python_path.as_ref() == Some(&command[0]) { Some("python".to_string()) }
                            else if profile.shell_path.as_ref() == Some(&command[0]) { Some("shell".to_string()) }
                            else { profile.toolchain_paths.iter().find(|(_,path)| *path == &command[0]).map(|(name,_)|name.clone()) }
                        })
                    }).collect::<Vec<_>>();
                    let mut validated = std::collections::HashSet::new();
                    for (command, capability) in commands.iter().zip(&capabilities) {
                        if let Some(capability) = capability && validated.insert(capability.clone()) {
                            super::validate_profile(&request, profiles.as_ref(), capability, Some(command[0].clone()), profile.as_ref().map(|profile|profile.shell_kind.clone())).await?;
                        }
                    }
                    if args["run_in_background"] == true {
                        request.native_argv = Some(commands.remove(0));
                        if let (Some(sandbox),Some(policy)) = (&sandbox, &request.sandbox_policy) { sandbox.prepare(policy).await.map_err(super::shell_runtime_failure)?; }
                        let spec = shell.resolve(request);
                        mark_effects();
                        let id = jobs.start(JobStart { kind:"native".into(), label:args["description"].as_str().unwrap_or("native process").into(), output_limit_bytes:None, owner,
                            run:Arc::new(move || Arc::new(super::PwshJobHooks { process:shell.start(spec.clone()), profiles:profiles.clone(), execution_context_id:spec.execution_context_id.clone(), capability:capabilities[0].clone() }))
                        }).map_err(ToolBodyError::plain)?;
                        return Ok(json!({"kind":"background","jobId":id.as_str(),"completion":"running","executionId":call_id}));
                    }
                    let total = commands.len();
                    if let (Some(sandbox),Some(policy))=(&sandbox,&request.sandbox_policy){sandbox.prepare(policy).await.map_err(super::shell_runtime_failure)?;}
                    let mut results = Vec::new();
                    for (index, command) in commands.into_iter().enumerate() {
                        if signal() { return Err(ToolBodyError::coded(format!("Execution cancelled before step {}; no further steps dispatched. Previous results: {}", index+1, Value::Array(results)), "AbortError", "SHELL_ABORTED")); }
                        let mut step = request.clone();
                        step.native_argv = Some(command.clone());
                        mark_effects();
                        let result = match shell.run(shell.resolve(step)).await {
                            Ok(result)=>result,
                            Err(error)=> {
                                if let (Some(profiles),Some(context),Some(capability)) = (&profiles,&request.execution_context_id,&capabilities[index]) { profiles.report_failure(context,capability); }
                                let mut failure = super::shell_runtime_failure(error);
                                failure.message = format!("{}\nStep {} has no completed execution result; {} dependent steps were not dispatched. Inspect possible effects before retrying.\nPrevious results: {}", failure.message,index+1,total-index-1,Value::Array(results));
                                return Err(failure);
                            },
                        };
                        let mut value = super::output::result_json(&result, &format!("{call_id}:{}",index+1));
                        if total == 1 { value["retryContext"] = json!({"commands": request.native_argv, "command":command,"workdir":request.workdir,"context":request.execution_context_id,"policy":request.sandbox_policy.as_ref().map(|p|format!("{:?}",p))}); }
                        let succeeded = value["completion"] == "succeeded";
                        if !succeeded {
                            if let (Some(profiles),Some(context),Some(capability)) = (&profiles,&request.execution_context_id,&capabilities[index]) { profiles.report_failure(context,capability); }
                        }
                        results.push(value.clone());
                        if !succeeded || index + 1 == total {
                            let mut value = value;
                            value["stdout"] = Value::String(results.iter().enumerate().map(|(index,value)| format!("[step {}]\n{}", index+1, super::output::render_result(value))).collect::<Vec<_>>().join("\n"));
                            if !succeeded {
                                let text = value["stdout"].as_str().unwrap_or_default().to_string();
                                value["stdout"] = Value::String(format!("{text}\n[{} dependent steps not dispatched; inspect existing effects before retry]", total-index-1));
                            }
                            value["steps"] = Value::Array(results);
                            if !succeeded && (args["allow_nonzero"] != true || result.aborted || result.timed_out) {
                                let code = if result.aborted {"SHELL_ABORTED"} else if result.timed_out {"SHELL_TIMEOUT"} else {"SHELL_FAILED"};
                                return Err(ToolBodyError::coded(super::output::render_result(&value), "ExecutionError", code).with_receipt(value));
                            }
                            return Ok(value);
                        }
                    }
                    unreachable!("nonempty steps return their final outcome")
                })
            }),
            finalize_content:None, present_call:None, present_result:None,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_is_data_and_unknown_profile_refs_fail_closed() {
        let parsed =
            argv(&json!({"program":"python","argv":["-c","print(\"a b\")","", "中文"]})).unwrap();
        assert_eq!(parsed[2], "print(\"a b\")");
        assert_eq!(parsed[3], "");
        let mut missing = vec!["@python".into()];
        assert!(resolve_program(&mut missing, None).is_err());
    }

    #[test]
    fn script_language_mismatch_fails_instead_of_reinterpreting_source() {
        let profile = dsh_shell::ResolvedExecutionProfile {
            shell_kind: "bash".into(),
            shell_path: Some("/selected/bash".into()),
            ..Default::default()
        };
        assert!(
            script_argv(
                &json!({"language":"powershell","script":"Get-Item ."}),
                Some(&profile)
            )
            .is_err()
        );
        let source = "printf '%s' 'literal $value; & quotes'";
        let arguments =
            script_argv(&json!({"language":"bash","script":source}), Some(&profile)).unwrap();
        assert_eq!(
            arguments,
            vec!["/selected/bash", "--noprofile", "--norc", "-c", source]
        );
    }
}
