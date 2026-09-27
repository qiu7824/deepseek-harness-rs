use super::*;
use dsh_agent::Agent;
use dsh_llm::{LlmAdapter, RequestAuthentication, RequestAuthenticationIdentity};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct BoundAdapter {
    authentication: RequestAuthentication,
    calls: Arc<AtomicUsize>,
}
impl LlmAdapter for BoundAdapter {
    fn request_authentication(&self) -> RequestAuthentication {
        self.authentication.clone()
    }
    fn stream(&self, _: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::pending())
    }
}
struct SelectedAdapter {
    selected: Arc<parking_lot::Mutex<RequestAuthentication>>,
    calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl LlmAdapter for SelectedAdapter {
    async fn snapshot_for_call(
        &self,
        _: &str,
        _: &str,
        _: Option<&Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<Option<Arc<dyn LlmAdapter>>, dsh_llm::LlmError> {
        Ok(Some(Arc::new(BoundAdapter {
            authentication: self.selected.lock().clone(),
            calls: self.calls.clone(),
        })))
    }
    fn stream(&self, _: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        unreachable!()
    }
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn logout_saved_a_stops_only_a_on_the_same_model_route_and_leaves_b_and_api_key_running() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    let a = model_tests::tokens("account-a");
    let b = model_tests::tokens("account-b");
    auth.activate(p, &a).await.unwrap();
    let authentication_a = auth
        .capture_request_authentication(p.id, Some(&a.account_scope))
        .await
        .unwrap();
    auth.activate(p, &b).await.unwrap();
    let authentication_b = auth
        .capture_request_authentication(p.id, Some(&b.account_scope))
        .await
        .unwrap();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    let sessions = dsh_session::SessionStore::install(&ctx);
    dsh_session_persistence_jsonl::JsonlSessionPersistence::install(
        &ctx,
        dsh_session_persistence_jsonl::JsonlConfig {
            root: root.join("sessions").to_string_lossy().into(),
            ..Default::default()
        },
    )
    .unwrap();
    let agents = ctx
        .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
        .unwrap()
        .as_ref()
        .clone();
    let llm = dsh_llm::LlmRuntime::install(&ctx);
    dsh_agent_loop::AgentLoop::install(&ctx, Default::default()).unwrap();
    let selected = Arc::new(parking_lot::Mutex::new(authentication_a.clone()));
    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["shared-route".into()],
        Arc::new(SelectedAdapter {
            selected: selected.clone(),
            calls: calls.clone(),
        }),
    )
    .unwrap();
    let mut running = Vec::new();
    for (index, authentication) in [
        authentication_a.clone(),
        authentication_b,
        RequestAuthentication::new(RequestAuthenticationIdentity::ApiKey),
    ]
    .into_iter()
    .enumerate()
    {
        *selected.lock() = authentication;
        let session = sessions
            .create(
                &ctx,
                Some(dsh_session::session_id(format!("logout-owner-{index}"))),
                None,
            )
            .await
            .unwrap();
        let agent = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session.id().clone(),
            dsh_agent::AgentOptions {
                provider: Some("shared-route".into()),
                model: Some("same-model".into()),
                ..Default::default()
            },
            session,
        )
        .unwrap();
        agents.enter(agent.clone(), None).unwrap();
        agent.followup(dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "synthetic task".into(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ));
        until(|| calls.load(Ordering::SeqCst) == index + 1).await;
        running.push(agent);
    }
    let impact = auth
        .handle(
            "logout-impact",
            &json!({"provider":p.id,"accountScope":a.account_scope}),
        )
        .await
        .unwrap();
    assert_eq!(impact["taskCount"], 1);
    let response = auth.handle("logout", &impact).await.unwrap();
    assert_eq!(response["cancelledTaskCount"], 1);
    assert_eq!(response["reason"], "account-signed-out");
    until(|| running[0].status() == dsh_agent::AgentStatus::Idle).await;
    assert_eq!(running[1].status(), dsh_agent::AgentStatus::Running);
    assert_eq!(running[2].status(), dsh_agent::AgentStatus::Running);
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        b.account_scope
    );
    assert!(authentication_a.is_revoked());
    for agent in running {
        agent.cancel(dsh_agent::AgentCancelCause::User, None);
        agent.when_idle().await;
    }
    auth.credentials.drain().await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn logout_confirmation_targets_captured_scope_and_relogin_cannot_reuse_a_generation() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    let a = model_tests::tokens("account-a");
    let b = model_tests::tokens("account-b");
    auth.activate(p, &a).await.unwrap();
    let before = auth
        .capture_request_authentication(p.id, Some(&a.account_scope))
        .await
        .unwrap();
    let impact = auth
        .handle("logout-impact", &json!({"provider":p.id}))
        .await
        .unwrap();
    auth.activate(p, &b).await.unwrap();
    assert_eq!(
        auth.handle("logout", &impact).await.unwrap()["removedAccountScope"],
        a.account_scope
    );
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        b.account_scope
    );
    assert!(before.is_revoked());
    auth.activate(p, &a).await.unwrap();
    let after = auth
        .capture_request_authentication(p.id, Some(&a.account_scope))
        .await
        .unwrap();
    assert_ne!(before.identity(), after.identity());
    assert!(!after.is_revoked());
    assert!(
        auth.handle("logout", &impact)
            .await
            .unwrap_err()
            .contains("重新确认")
    );
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        a.account_scope
    );
    assert!(
        auth.resolve_request_token_for_authentication(
            p.id,
            p.base,
            &[],
            Some(&a.account_scope),
            &before
        )
        .await
        .is_err()
    );
    assert!(
        auth.handle("logout", &json!({"provider":p.id}))
            .await
            .unwrap_err()
            .contains("accountScope")
    );
    assert!(
        auth.handle(
            "logout",
            &json!({"provider":p.id,"accountScope":a.account_scope})
        )
        .await
        .unwrap_err()
        .contains("loginGeneration")
    );
    auth.credentials.drain().await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn frozen_native_oauth_route_exposes_only_non_secret_identity_and_keeps_scope_guard() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    let a = model_tests::tokens("account-a");
    let b = model_tests::tokens("account-b");
    auth.activate(p, &a).await.unwrap();
    let profile = auth.profile_snapshot(p.id, false).unwrap().0;
    let config: crate::OpenAiCompatibleProviderConfig = serde_json::from_value(profile).unwrap();
    let adapter = crate::OpenAiCompatibleAdapter {
        auth: Some(auth.clone()),
        credentials: auth.credentials.clone(),
        profiles: Arc::new(parking_lot::Mutex::new(indexmap::IndexMap::from([(
            p.id.into(),
            config,
        )]))),
        attachment_ctx: ctx.clone(),
    };
    let frozen = adapter
        .snapshot_for_call(p.id, "synthetic-model", None)
        .await
        .unwrap()
        .unwrap();
    let identity = frozen.request_authentication();
    let text = serde_json::to_string(identity.identity()).unwrap();
    assert!(text.contains(&a.account_scope));
    assert!(!text.contains(&a.access_token) && !text.contains("fixture-refresh"));
    auth.activate(p, &b).await.unwrap();
    let headers = vec![("chatgpt-account-id".into(), a.account_id.clone().unwrap())];
    assert!(
        auth.resolve_request_token_for_scope(p.id, p.base, &headers, Some(&a.account_scope))
            .await
            .is_err()
    );
    assert!(
        auth.resolve_request_token_for_authentication(
            p.id,
            p.base,
            &headers,
            Some(&a.account_scope),
            &identity
        )
        .await
        .is_err()
    );
    auth.credentials.drain().await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn logout_impact_without_a_live_registry_is_unknown_not_zero() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    auth.activate(p, &model_tests::tokens("account-a"))
        .await
        .unwrap();
    *auth.agents.write() = std::sync::Weak::new();
    assert!(
        auth.handle("logout-impact", &json!({"provider":p.id}))
            .await
            .unwrap_err()
            .contains("未知")
    );
    auth.credentials.drain().await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn failed_credential_commit_keeps_the_authorization_lease_live() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    let account = model_tests::tokens("account-a");
    auth.activate(p, &account).await.unwrap();
    let captured = auth
        .capture_request_authentication(p.id, Some(&account.account_scope))
        .await
        .unwrap();
    let impact = auth
        .handle("logout-impact", &json!({"provider":p.id}))
        .await
        .unwrap();
    auth.credentials.drain().await;
    assert!(auth.handle("logout", &impact).await.is_err());
    assert!(!captured.is_revoked());
    assert_eq!(
        auth.session(p.id).await.unwrap().unwrap().account_scope,
        account.account_scope
    );
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}
