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
        "write" | "edit" | "write_file" | "patch" | "apply_patch" | "workspace_scratch" => {
            EffectClass::Write
        }
        "generate_image" | "send_message" => EffectClass::External,
        _ => EffectClass::Unknown,
    }
}

fn snapshot(execution: &ToolExecution) -> Result<Value, String> {
    let Some(agent) = &execution.agent else {
        return Ok(json!({"version":1,"scope":"host"}));
    };
    let ctx = agent.ctx();
    let session = agent.session();
    let workspace = session.header().cwd.clone();
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
        .map(|service| {
            service.try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
                session: Some(Arc::new(session.clone())),
                mode: None,
            })
        })
        .transpose()?;
    let profile = workdir.as_deref().and_then(|cwd| {
        ctx.get_typed::<Arc<dyn dsh_shell::ExecutionProfileResolver>>("executionProfiles", false)
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
        let backend=ctx.get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox",false)
            .map(|provider|provider.backend_fingerprint_for(policy));
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
    let mut value = json!({"version":1,"sessionId":agent.id().as_str(),"workspace":workspace,"workingDirectory":workdir,
        "nativeRoute":native_route,"selectedPolicy":selected,"profile":profile,"requestedPermissions":execution.arguments.get("sandbox_permissions")});
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
    #[tokio::test]
    async fn changing_wrappers_does_not_reset_typed_environment_failures_but_real_probe_and_context_do()
     {
        let ctx = cordis::Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
        let epoch = Arc::new(AtomicUsize::new(0));
        let snapshot = epoch.clone();
        ctx.register_service(Arc::new(ExecutionEvidenceProvider{classify:Arc::new(|_|EffectClass::Unknown),snapshot:Arc::new(move |_|Ok(json!({"task":{"id":"task","requirementsRevision":snapshot.load(Ordering::SeqCst)},"policy":"workspace-write","profile":"one"})))}));
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
                    json!({"argument":round}),
                ))
                .await;
            assert!(result.is_error);
        }
        let result = tools
            .execute(input(
                "execute_native",
                json!({"argument":"another wrapper"}),
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
                json!({"argument":"requirements changed"}),
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
    }
}
