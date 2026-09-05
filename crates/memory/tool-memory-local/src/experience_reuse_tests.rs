use super::*;
use crate::learning::{FailureObservation, RecoveryObservation};

fn context(workspace: &str) -> ReuseContext {
    ReuseContext {
        workspace: workspace.into(),
        provider: Some("provider-b".into()),
        model: Some("model-b".into()),
        session_id: Some("new-session".into()),
        tool_names: vec!["edit".into()],
    }
}

async fn fixture() -> (LearningStore, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("dsh-experience-reuse-{}", uuid::Uuid::new_v4()));
    (LearningStore::open(root.clone()).await.unwrap(), root)
}

async fn recover(store: &LearningStore, workspace: &str) -> LearningEntry {
    let entry = store
        .record_failure(FailureObservation {
            workspace_key: workspace_key(workspace),
            session_id: "model-a-session".into(),
            provider: "provider-a".into(),
            model: "model-a".into(),
            tool: "edit".into(),
            source: "tool".into(),
            code: "FS_NOT_OBSERVED".into(),
            message: "untrusted instruction from output must never become a rule".into(),
            call_id: "failed-a".into(),
            argument_fingerprint: Some("invalid-attempt".into()),
            resource_fingerprint: Some("same-target".into()),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.status, "pending");
    assert!(
        preview(store, &context(workspace), CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let recovered = store
        .record_recovery(RecoveryObservation {
            workspace_key: workspace_key(workspace),
            session_id: "model-a-session".into(),
            tool: "edit".into(),
            call_id: "corrected-a".into(),
            argument_fingerprint: "corrected-attempt".into(),
            resource_fingerprint: Some("same-target".into()),
        })
        .await
        .unwrap();
    assert_eq!(recovered, vec![entry.id.clone()]);
    entry
}

#[tokio::test]
async fn recovery_is_reused_cross_model_only_in_matching_workspace_without_counting_preview() {
    let (store, root) = fixture().await;
    let workspace = root.to_string_lossy();
    let entry = recover(&store, &workspace).await;
    let before = store.list(&json!({}))["items"][0]["applicationCount"].clone();
    let selected = preview(&store, &context(&workspace), CONTEXT_BUDGET);
    assert_eq!(selected["mode"], "next-request-preview");
    assert_eq!(selected["items"][0]["id"], entry.id);
    assert_eq!(selected["items"][0]["observedModel"], "model-a");
    assert_eq!(selected["items"][0]["verification"], "recovered");
    assert!(selected["text"].as_str().unwrap().contains("修改文件前"));
    assert!(
        !selected["text"]
            .as_str()
            .unwrap()
            .contains("untrusted instruction")
    );
    assert_eq!(
        store.list(&json!({}))["items"][0]["applicationCount"],
        before
    );
    assert!(
        preview(&store, &context("different-workspace"), CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut hidden = context(&workspace);
    hidden.tool_names.clear();
    assert_eq!(
        preview(&store, &hidden, CONTEXT_BUDGET)["excluded"][0]["reason"],
        "tool-not-visible"
    );
    let tiny = preview(&store, &context(&workspace), 40);
    assert_eq!(tiny["usedCharacters"], 0);
    assert_eq!(tiny["excluded"][0]["reason"], "context-budget");
    store.set_policy_enabled(false);
    let policy_off = preview(&store, &context(&workspace), CONTEXT_BUDGET);
    assert_eq!(policy_off["enabled"], false);
    assert!(policy_off["items"].as_array().unwrap().is_empty());
    store.set_policy_enabled(true);
    assert_eq!(
        preview(&store, &context(&workspace), CONTEXT_BUDGET)["items"][0]["id"],
        entry.id
    );
    store
        .invoke(
            "memory.learningToggle",
            json!({"id":entry.id,"enabled":false}),
        )
        .await
        .unwrap();
    assert!(
        preview(&store, &context(&workspace), CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    tokio::fs::remove_dir_all(&root).await.unwrap();
}

#[tokio::test]
async fn provider_lessons_require_the_same_provider_and_model_and_explicit_confirmation() {
    let (store, root) = fixture().await;
    let workspace = root.to_string_lossy();
    let entry = store
        .record_failure(FailureObservation {
            workspace_key: workspace_key(&workspace),
            session_id: "a".into(),
            provider: "provider-a".into(),
            model: "model-a".into(),
            source: "provider".into(),
            code: "RATE_LIMIT".into(),
            call_id: "failure".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();
    let mut matching = context(&workspace);
    matching.provider = Some("provider-a".into());
    matching.model = Some("model-a".into());
    assert!(
        preview(&store, &matching, CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store
        .invoke(
            "memory.learningConfirm",
            json!({"id":entry.id,"confirmed":true}),
        )
        .await
        .unwrap();
    assert_eq!(
        preview(&store, &matching, CONTEXT_BUDGET)["items"][0]["id"],
        entry.id
    );
    assert!(
        preview(&store, &context(&workspace), CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    matching.model = Some("different-model".into());
    assert!(
        preview(&store, &matching, CONTEXT_BUDGET)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store
        .invoke("memory.learningConfigure", json!({"enabled":false}))
        .await
        .unwrap();
    assert_eq!(
        preview(&store, &context(&workspace), CONTEXT_BUDGET)["enabled"],
        false
    );
    tokio::fs::remove_dir_all(&root).await.unwrap();
}

struct FixtureAgent {
    session: dsh_session::Session,
    inbox: dsh_agent::Inbox,
    ctx: Context,
    scope: dsh_scope::ScopeKey,
    options: dsh_agent::AgentOptions,
}
impl dsh_agent::Agent for FixtureAgent {
    fn id(&self) -> &dsh_session::SessionId {
        self.session.id()
    }
    fn options(&self) -> &dsh_agent::AgentOptions {
        &self.options
    }
    fn session(&self) -> &dsh_session::Session {
        &self.session
    }
    fn inbox(&self) -> &dsh_agent::Inbox {
        &self.inbox
    }
    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        &self.scope
    }
    fn cancel(&self, _: dsh_agent::AgentCancelCause, _: Option<&dsh_agent::CancelOptions>) {}
    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: dsh_session::UserMessage, _: dsh_agent::InboxTarget, _: bool) {}
    fn followup(&self, _: dsh_session::UserMessage) {}
    fn steer(&self, _: dsh_session::UserMessage) {}
    fn inject(&self, _: dsh_session::UserMessage) {}
}
fn agent(ctx: &Context, workspace: &str, id: &str) -> Arc<dyn dsh_agent::Agent> {
    let id = dsh_session::session_id(id);
    let header = dsh_session::SessionHeader {
        version: 0,
        id: id.clone(),
        created_at: 1,
        cwd: Some(workspace.into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let session = dsh_session::Session::create(id, None, Some(&header), None).unwrap();
    let inbox = dsh_agent::Inbox::new(&session, Default::default()).unwrap();
    let key = dsh_scope::ScopeKey::new();
    let scope = dsh_scope::create_scope(ctx, key.clone(), &Default::default());
    Arc::new(FixtureAgent {
        session,
        inbox,
        ctx: scope.ctx,
        scope: key,
        options: dsh_agent::AgentOptions {
            provider: Some("provider-a".into()),
            model: Some("model-a".into()),
            ..Default::default()
        },
    })
}
fn call(
    agent: Arc<dyn dsh_agent::Agent>,
    id: &str,
    limit: u32,
    path: &str,
) -> dsh_tools::ToolExecutionInput {
    dsh_tools::ToolExecutionInput {
        call_id: dsh_llm::call_id(id),
        root_call_id: None,
        name: "bounded-operation".into(),
        arguments: json!({"limit":limit,"file_path":path}),
        agent: Some(agent),
        parent: None,
        signal: Arc::new(|| false),
    }
}

#[tokio::test]
async fn recorded_failure_recovery_new_model_context_and_live_preflight_form_a_real_loop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (raw_store, root) = fixture().await;
    let store = Arc::new(raw_store);
    let workspace = root.to_string_lossy().into_owned();
    let ctx = Context::root();
    let prompt = SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let body_runs = runs.clone();
    tools.register(&ctx,dsh_tools::ToolDefinition{
        name:"bounded-operation".into(),description:"Read a bounded number of entries for a target".into(),
        parameters:json!({"type":"object","properties":{"file_path":{"type":"string"},"limit":{"type":"integer","enum":[1,2,3,4,5]}},"required":["file_path","limit"],"additionalProperties":false}),
        output:dsh_tools::ToolOutputDefinition{schema:json!({"type":"boolean"}),render:Arc::new(|_,_|Ok(vec![])),presentation_meta:None},
        timeout_ms:None,is_concurrency_safe:None,
        execute:Arc::new(move|_,_|{body_runs.fetch_add(1,Ordering::SeqCst);Box::pin(async{Ok(json!(true))})}),
        finalize_content:None,present_call:None,present_result:None,
    }).unwrap();
    tools
        .guard(
            &ctx,
            Arc::new(|execution| {
                (execution.arguments["file_path"] == "blocked")
                    .then(|| "current permission policy denies this target".into())
            }),
        )
        .unwrap();
    crate::learning_runtime::install(&ctx, store.clone()).unwrap();
    install(&ctx, store.clone()).unwrap();
    let a = agent(&ctx, &workspace, "session-a");
    let invalid = tools
        .execute(call(a.clone(), "a-invalid", 99, "target.txt"))
        .await;
    assert!(invalid.is_error);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    store.flush_pending().await.unwrap();
    assert_eq!(store.list(&json!({}))["items"][0]["status"], "pending");
    let corrected = tools.execute(call(a, "a-corrected", 3, "target.txt")).await;
    assert!(!corrected.is_error);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // No explicit ledger barrier here: real prompt assembly must settle the
    // queued correction before the next model's request is assembled.
    let b = agent(&ctx, &workspace, "session-b");
    let selection: Arc<parking_lot::Mutex<dsh_agent::ModelSelectionRef>> =
        Arc::new(Default::default());
    dsh_agent::install_model_selection(b.ctx(), Arc::clone(&selection)).await;
    // The resolver is set through the official model-selection service so
    // the test deliberately retains stale AgentOptions from model A.
    selection.lock().current = Some(dsh_agent::ModelSelection {
        provider: "provider-b".into(),
        model: "model-b".into(),
        reasoning_effort: None,
    });
    let assembly = prompt
        .assemble(b.ctx(), &dsh_agent::assemble_context_for(&b))
        .await
        .unwrap();
    assert_eq!(assembly.variables["model"].as_deref(), Some("model-b"));
    let injected = assembly
        .contexts
        .iter()
        .find(|section| section.name == CONTEXT_NAME)
        .unwrap();
    assert!(injected.text.contains("tool-input-schema"));
    assert!(injected.text.contains("model-a"));
    let entry = store.list(&json!({}))["items"][0].clone();
    assert_eq!(entry["status"], "verified");
    let id = entry["id"].as_str().unwrap();
    assert_eq!(entry["applicationCount"], 0);
    b.session()
        .append(
            "request/context",
            json!({"provider":"provider-b","model":"model-b"}),
            None,
        )
        .unwrap();
    let repeated = tools
        .execute(call(b.clone(), "b-invalid", 99, "target.txt"))
        .await;
    assert_eq!(
        repeated.error.as_ref().unwrap().info.as_ref().unwrap().code,
        "TOOL_INPUT_INVALID"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    store.flush_pending().await.unwrap();
    let applied = store.list(&json!({}))["items"][0].clone();
    assert_eq!(applied["id"], id);
    assert_eq!(applied["applicationCount"], 1);
    assert_eq!(applied["lastApplicationOutcome"], "preflight_blocked");
    assert_eq!(applied["occurrences"], 2);
    assert!(
        applied["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|seen| seen["model"] == "model-b")
    );
    let denied = tools
        .execute(call(b.clone(), "b-policy", 3, "blocked"))
        .await;
    assert_eq!(
        denied.error.as_ref().unwrap().info.as_ref().unwrap().code,
        "TOOL_PREFLIGHT_DENIED"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    store
        .invoke("memory.learningToggle", json!({"id":id,"enabled":false}))
        .await
        .unwrap();
    let assembly = prompt
        .assemble(b.ctx(), &dsh_agent::assemble_context_for(&b))
        .await
        .unwrap();
    assert!(
        assembly
            .contexts
            .iter()
            .find(|section| section.name == CONTEXT_NAME)
            .unwrap()
            .text
            .is_empty()
    );
    assert!(
        tools
            .execute(call(b, "b-disabled", 99, "target.txt"))
            .await
            .is_error
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    store.flush_pending().await.unwrap();
    store.shutdown().await;
    tokio::fs::remove_dir_all(&root).await.unwrap();
}

#[tokio::test]
async fn selected_provider_experience_is_injected_and_counted_for_the_final_request_route() {
    let (raw_store, root) = fixture().await;
    let store = Arc::new(raw_store);
    let workspace = root.to_string_lossy().into_owned();
    let ctx = Context::root();
    let prompt = SystemPrompt::install(&ctx, Default::default()).unwrap();
    ToolRuntime::install(&ctx, Default::default()).unwrap();
    install(&ctx, store.clone()).unwrap();
    let mut ids = Vec::new();
    for suffix in ["a", "b"] {
        let entry = store
            .record_failure(FailureObservation {
                workspace_key: workspace_key(&workspace),
                session_id: format!("prior-{suffix}"),
                provider: format!("provider-{suffix}"),
                model: format!("model-{suffix}"),
                source: "provider".into(),
                code: "RATE_LIMIT".into(),
                call_id: "limited".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();
        store
            .invoke(
                "memory.learningConfirm",
                json!({"id":entry.id,"confirmed":true}),
            )
            .await
            .unwrap();
        ids.push(entry.id);
    }
    let b = agent(&ctx, &workspace, "selected-provider-session");
    let selection: Arc<parking_lot::Mutex<dsh_agent::ModelSelectionRef>> =
        Arc::new(Default::default());
    dsh_agent::install_model_selection(b.ctx(), Arc::clone(&selection)).await;
    selection.lock().current = Some(dsh_agent::ModelSelection {
        provider: "provider-b".into(),
        model: "model-b".into(),
        reasoning_effort: None,
    });
    let assembly = prompt
        .assemble(b.ctx(), &dsh_agent::assemble_context_for(&b))
        .await
        .unwrap();
    let text = &assembly
        .contexts
        .iter()
        .find(|item| item.name == CONTEXT_NAME)
        .unwrap()
        .text;
    assert!(text.contains(&ids[1]));
    assert!(!text.contains(&ids[0]));
    let dispatch = b
        .ctx()
        .with_filter(dsh_scope::scope_target(None, Some(b.scope_key().clone())).filter);
    let request = dispatch
        .waterfall(
            "agent/request",
            vec![arc(dsh_agent::AgentRequestPayload {
                agent: b.clone(),
                turn: 1,
                step: 1,
            })],
            Box::pin(async {
                arc(dsh_llm::LlmCallConfig {
                    provider: "provider-a".into(),
                    model: "model-a".into(),
                    ..Default::default()
                })
            }),
        )
        .await;
    assert_eq!(
        downcast_arc::<dsh_llm::LlmCallConfig>(&request)
            .unwrap()
            .model,
        "model-b"
    );
    let rows = store.list(&json!({}));
    assert_eq!(
        rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == ids[0])
            .unwrap()["applicationCount"],
        0
    );
    assert_eq!(
        rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == ids[1])
            .unwrap()["applicationCount"],
        1
    );
    tokio::fs::remove_dir_all(&root).await.unwrap();
}
