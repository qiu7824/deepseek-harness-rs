//! Non-secret, immutable admission identities shared by all tool adapters.
use dsh_tools::{
    ToolExecution,
    receipt::{EffectClass, ExecutionEvidenceProvider},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

pub(super) fn classify(execution: &ToolExecution) -> EffectClass {
    if crate::task_effects::read_only(&execution.name, &execution.arguments) {
        return EffectClass::ReadOnly;
    }
    match execution.name.as_str() {
        "write" | "edit" | "write_file" | "patch" | "apply_patch" | "workspace_scratch"
        | "office_write" | "file_manage" => EffectClass::Write,
        "generate_image" | "send_message" => EffectClass::External,
        _ => EffectClass::Unknown,
    }
}

fn uses_execution_environment(name: &str) -> bool {
    matches!(
        name,
        "pwsh"
            | "bash"
            | "execute_native"
            | "execute_script"
            | "execute_steps"
            | "run_code"
            | "terminal_open"
            | "terminal_send"
            | "environment_validate"
    )
}

fn snapshot(execution: &ToolExecution) -> Result<Value, String> {
    let Some(agent) = &execution.agent else {
        return Ok(json!({"version":1,"scope":"host"}));
    };
    let ctx = agent.ctx();
    let session = agent.session();
    let workspace = session.header().cwd.clone();
    let depends_on_runtime = uses_execution_environment(&execution.name);
    let route_only = matches!(
        execution.name.as_str(),
        "generate_image" | "web_search" | "consult_model"
    );
    let workdir = execution.arguments["workdir"]
        .as_str()
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                Path::new(workspace.as_deref().unwrap_or(".")).join(path)
            }
        })
        .map(|path| {
            std::fs::canonicalize(&path)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        })
        .or_else(|| workspace.clone());
    let policy = ctx
        .get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
        .filter(|_| !route_only)
        .map(|service| {
            service.try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
                session: Some(Arc::new(session.clone())),
                mode: None,
            })
        })
        .transpose()?;
    let profile = depends_on_runtime
        .then_some(())
        .and_then(|_| workdir.as_deref())
        .and_then(|cwd| {
            ctx.get_typed::<Arc<dyn dsh_shell::ExecutionProfileResolver>>(
                "executionProfiles",
                false,
            )
            .map(|service| service.resolve(Some(agent.id().as_str()), cwd))
        });
    let profile = match profile {
        Some(Ok(profile)) => {
            json!({"contextId":profile.context_id,"shell":profile.shell_path,"shellKind":profile.shell_kind,"python":profile.python_path,"toolchains":profile.toolchain_paths})
        }
        Some(Err(error)) => json!({"state":"unavailable","reason":error}),
        None => Value::Null,
    };
    let selected=policy.as_ref().map(|policy| {
        let mut roots=policy.read_only_roots.clone();roots.sort();roots.dedup();
        let backend=depends_on_runtime.then(||ctx.get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox",false)
            .map(|provider|provider.backend_fingerprint_for(policy))).flatten();
        json!({"mode":policy.mode.as_str(),"workspace":policy.workspace_root,"readOnlyRoots":roots,"backend":backend})
    });
    let role = match execution.name.as_str() {
        "generate_image" => Some("image"),
        "web_search" => Some("search"),
        "consult_model" => execution.arguments["task"].as_str(),
        _ => None,
    };
    let native_route = role.and_then(|role| {
        ctx.get_typed::<Arc<crate::task_models::TaskModels>>("taskModels", false)
            .map(|models| models.admission_identity(role, agent))
    });
    let recovery_profile = depends_on_runtime
        .then(|| {
            workspace.as_deref().and_then(|cwd| {
                ctx.get_typed::<Arc<dyn dsh_shell::ExecutionProfileResolver>>(
                    "executionProfiles",
                    false,
                )
                .and_then(|service| service.recovery_identity(Some(agent.id().as_str()), cwd))
            })
        })
        .flatten();
    let mut value = json!({"version":1,"sessionId":agent.id().as_str(),"workspace":workspace,"workingDirectory":workdir,
        "nativeRoute":native_route,"selectedPolicy":selected,"profile":profile,"requestedPermissions":execution.arguments.get("sandbox_permissions")});
    if depends_on_runtime {
        value["startupRecoveryIdentity"] = json!({"sessionId":agent.id().as_str(),"workspace":workspace,"profileChoice":recovery_profile});
    }
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).map_err(|error| error.to_string())?)
    );
    value["fingerprint"] = json!(fingerprint);
    Ok(value)
}

pub(super) fn install(ctx: &cordis::Context) {
    ctx.register_service(Arc::new(ExecutionEvidenceProvider {
        classify: Arc::new(classify),
        snapshot: Arc::new(snapshot),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_tools::{
        ToolBodyError, ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct MutableProfile {
        epoch: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
    }
    impl dsh_shell::ExecutionProfileResolver for MutableProfile {
        fn resolve(
            &self,
            _: Option<&str>,
            _: &str,
        ) -> Result<dsh_shell::ResolvedExecutionProfile, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(dsh_shell::ResolvedExecutionProfile {
                context_id: self.epoch.load(Ordering::SeqCst).to_string(),
                shell_path: Some("selected-shell.exe".into()),
                shell_kind: "powershell".into(),
                ..Default::default()
            })
        }
    }
    #[tokio::test]
    async fn shell_changes_do_not_gate_files_office_or_routes_but_execution_still_checks_them() {
        let root = std::env::temp_dir().join(format!(
            "dsh-evidence-dependencies-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (name, change_policy) in [
            ("write", false),
            ("workspace_scratch", false),
            ("office_read", false),
            ("office_write", false),
            ("office_render", false),
            ("file_manage", false),
            ("generate_image", false),
            ("web_search", false),
            ("execute_native", false),
            ("write", true),
            ("workspace_scratch", true),
            ("generate_image", true),
            ("web_search", true),
        ] {
            let ctx = cordis::Context::root();
            dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
            dsh_llm::LlmRuntime::install(&ctx);
            let epoch = Arc::new(AtomicUsize::new(0));
            let reads = Arc::new(AtomicUsize::new(0));
            ctx.register_service(Arc::new(MutableProfile {
                epoch: epoch.clone(),
                reads: reads.clone(),
            })
                as Arc<dyn dsh_shell::ExecutionProfileResolver>);
            dsh_sandbox_policy::SandboxPolicyService::install(
                &ctx,
                dsh_sandbox_policy::Config {
                    mode: Some(dsh_sandbox::SandboxMode::WorkspaceWrite),
                    workspace_root: Some(root.to_string_lossy().into_owned()),
                },
            );
            let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
            install(&ctx);
            let changing = epoch.clone();
            tools
                .register(
                    &ctx,
                    ToolDefinition {
                        name: name.into(),
                        description: "dependency boundary fixture".into(),
                        parameters: json!({"type":"object"}),
                        output: ToolOutputDefinition {
                            schema: json!({"type":"boolean"}),
                            render: Arc::new(|_, _| Ok(vec![])),
                            presentation_meta: None,
                        },
                        timeout_ms: None,
                        is_concurrency_safe: None,
                        finalize_content: None,
                        present_call: None,
                        present_result: None,
                        execute: Arc::new(move |_, run| {
                            let marker = run.track_cancellable_effects();
                            let changing = changing.clone();
                            let agent = run.agent.clone();
                            Box::pin(async move {
                                changing.fetch_add(1, Ordering::SeqCst);
                                if change_policy {
                                    dsh_sandbox_policy::set_sandbox_mode(
                                        agent.as_ref().unwrap().session(),
                                        dsh_sandbox::SandboxMode::ReadOnly,
                                    )
                                    .unwrap();
                                }
                                marker().map_err(ToolBodyError::plain)?;
                                Ok(json!(true))
                            })
                        }),
                    },
                )
                .unwrap();
            let store = dsh_session::SessionStore::install(&ctx);
            let session = store
                .create(
                    &ctx,
                    None,
                    Some(dsh_session::CreateSessionOptions {
                        meta: Some(dsh_session::CreateSessionMeta {
                            cwd: Some(root.to_string_lossy().into_owned()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                )
                .await
                .unwrap();
            let agent: Arc<dyn dsh_agent::Agent> = dsh_agent_loop::ReactLoopAgent::new(
                &ctx,
                session.id().clone(),
                Default::default(),
                session,
            )
            .unwrap();
            let result = tools
                .execute(ToolExecutionInput {
                    call_id: dsh_llm::call_id("dependency"),
                    root_call_id: None,
                    name: name.into(),
                    arguments: json!({}),
                    agent: Some(agent),
                    parent: None,
                    signal: Arc::new(|| false),
                })
                .await;
            if name == "execute_native"
                || (change_policy && !matches!(name, "generate_image" | "web_search"))
            {
                assert!(result.is_error);
                assert_eq!(
                    result.meta.as_ref().unwrap()["executionReceipt"]["errorCode"],
                    "EXECUTION_CONTEXT_CHANGED"
                );
                assert_eq!(
                    result.meta.as_ref().unwrap()["executionReceipt"]["effects"],
                    "none"
                );
            } else {
                assert!(!result.is_error, "{name}: {:?}", result.error);
                assert_eq!(
                    reads.load(Ordering::SeqCst),
                    0,
                    "{name} has no interpreter dependency"
                );
            }
            ctx.fiber.dispose().await;
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn changing_wrappers_does_not_reset_typed_environment_failures_but_real_probe_and_context_do()
     {
        let ctx = cordis::Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
        let epoch = Arc::new(AtomicUsize::new(0));
        let snapshot = epoch.clone();
        ctx.register_service(Arc::new(ExecutionEvidenceProvider{classify:Arc::new(|_|EffectClass::Unknown),snapshot:Arc::new(move |execution|Ok(json!({"startupRecoveryIdentity":{"profileRevision":snapshot.load(Ordering::SeqCst)},"workingDirectory":execution.arguments["workdir"],"policy":"workspace-write","profile":{"contextId":execution.arguments["workdir"],"shell":"one"}})))}));
        dsh_repeat_tool_reminder::progress::install(&ctx).await;
        let calls = Arc::new(AtomicUsize::new(0));
        for name in [
            "execute_native",
            "execute_script",
            "pwsh",
            "environment_validate",
            "native_cached_fixture",
        ] {
            let calls = calls.clone();
            let probe = name == "environment_validate";
            let cached = name == "native_cached_fixture";
            tools.register(&ctx,ToolDefinition{name:name.into(),description:"typed execution fixture".into(),parameters:json!({"type":"object"}),
                output:ToolOutputDefinition{schema:json!({}),render:Arc::new(|_,_|Ok(vec![])),presentation_meta:None},timeout_ms:None,is_concurrency_safe:None,finalize_content:None,present_call:None,present_result:None,
                execute:Arc::new(move |args,run| {let marker=run.track_cancellable_effects();let fresh=args["refresh"]==true;let attempt=calls.fetch_add(1,Ordering::SeqCst);
                    Box::pin(async move {if probe {return Ok(json!({"status":"ready","cacheHit":!fresh,"level":"launch","executionWorld":"selected_environment","checkId":if fresh{"new-proof"}else{"cached"}}));}
                    if cached {return Err(ToolBodyError::coded(format!("temporary cache countdown {attempt}"),"NativeCapabilityError","NATIVE_TOOL_UNSUPPORTED"));}
                    marker().map_err(ToolBodyError::plain)?;Err(ToolBodyError::coded("fixture setup failure","SandboxError","SANDBOX_SETUP_FAILED"))})})}).unwrap();
        }
        let session = dsh_session::Session::create(
            dsh_session::session_id("failure-owner"),
            None,
            None,
            None,
        )
        .unwrap();
        let agent: Arc<dyn dsh_agent::Agent> = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session.id().clone(),
            Default::default(),
            session,
        )
        .unwrap();
        let input = |name: &str, args: Value| ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: name.into(),
            arguments: args,
            agent: Some(agent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        };
        for round in 0..5 {
            let result = tools
                .execute(input(
                    ["pwsh", "execute_native", "execute_script"][round % 3],
                    json!({"argument":round,"workdir":format!("D:/documents/{round}")}),
                ))
                .await;
            assert!(result.is_error);
        }
        let result = tools
            .execute(input(
                "execute_native",
                json!({"argument":"another wrapper","workdir":"D:/totally-different"}),
            ))
            .await;
        assert_eq!(
            result.meta.as_ref().unwrap()["executionReceipt"]["bodyInvoked"],
            false
        );
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        tools
            .execute(input("environment_validate", json!({"refresh":false})))
            .await;
        tools
            .execute(input(
                "pwsh",
                json!({"argument":"cached proof is insufficient"}),
            ))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 6);
        tools
            .execute(input("environment_validate", json!({"refresh":true})))
            .await;
        tools
            .execute(input("pwsh", json!({"argument":"new launch evidence"})))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 8);
        for round in 0..4 {
            tools
                .execute(input("execute_script", json!({"argument":round})))
                .await;
        }
        epoch.store(1, Ordering::SeqCst);
        tools
            .execute(input(
                "execute_native",
                json!({"argument":"explicit profile revision changed"}),
            ))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 13);
        for _ in 0..5 {
            tools
                .execute(input("native_cached_fixture", json!({})))
                .await;
        }
        tools
            .execute(input("native_cached_fixture", json!({})))
            .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            18,
            "changing cache countdowns are not evidence of progress"
        );
        epoch.store(2, Ordering::SeqCst);
        tools
            .execute(input("native_cached_fixture", json!({})))
            .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            19,
            "a genuinely changed context permits the exact same call again"
        );
        for round in 0..5 {
            tools
                .execute(input("execute_native", json!({"freshFailure":round})))
                .await;
        }
        ctx.parallel(
            "agent/turn-finished",
            vec![cordis::arc(dsh_agent::AgentTurnStoppingPayload {
                agent: agent.clone(),
                turn: 1,
            })],
        )
        .await;
        ctx.parallel(
            "agent/pre-step",
            vec![cordis::arc(dsh_agent::AgentPreStepPayload {
                agent: agent.clone(),
                messages: vec![],
                turn: 2,
                step: 1,
                signal: dsh_agent::CancellationSignal::new(),
            })],
        )
        .await;
        tools
            .execute(input("pwsh", json!({"argument":"automatic continuation"})))
            .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            24,
            "turn boundaries alone do not repair startup"
        );
        ctx.parallel(
            "agent/pre-step",
            vec![cordis::arc(dsh_agent::AgentPreStepPayload {
                agent: agent.clone(),
                messages: vec![dsh_llm::create_user_message(
                    vec![dsh_llm::ContentBlock::Text {
                        text: "Retry with my updated environment".into(),
                    }],
                    dsh_llm::MessageSource::User {
                        rpc_id: None,
                        client_time_zone: None,
                    },
                )],
                turn: 2,
                step: 2,
                signal: dsh_agent::CancellationSignal::new(),
            })],
        )
        .await;
        tools
            .execute(input("pwsh", json!({"argument":"explicit user retry"})))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 25);
        ctx.fiber.dispose().await;
    }
}
