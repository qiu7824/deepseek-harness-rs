use super::*;
use dsh_agent::Agent;
use std::sync::atomic::{AtomicUsize, Ordering};

struct PendingAdapter {
    authentication: dsh_llm::RequestAuthentication,
    entered: Arc<tokio::sync::Notify>,
}
impl dsh_llm::LlmAdapter for PendingAdapter {
    fn request_authentication(&self) -> dsh_llm::RequestAuthentication {
        self.authentication.clone()
    }
    fn stream(&self, _: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        self.entered.notify_one();
        Box::pin(futures::stream::pending())
    }
}

#[tokio::test]
async fn logout_flush_failure_is_explicit_and_never_activates_the_remaining_account() {
    let (auth, ctx, root) = model_tests::setup().await;
    let p = provider("openai-codex").unwrap();
    let a = model_tests::tokens("account-a");
    let b = model_tests::tokens("account-b");
    auth.activate(p, &b).await.unwrap();
    auth.activate(p, &a).await.unwrap();
    let captured = auth
        .capture_request_authentication(p.id, Some(&a.account_scope))
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
    let entered = Arc::new(tokio::sync::Notify::new());
    llm.register_adapter(
        &ctx,
        vec!["fixture".into()],
        Arc::new(PendingAdapter {
            authentication: captured.clone(),
            entered: entered.clone(),
        }),
    )
    .unwrap();
    let session = sessions
        .create(
            &ctx,
            Some(dsh_session::session_id("logout-flush-failure")),
            None,
        )
        .await
        .unwrap();
    let agent = dsh_agent_loop::ReactLoopAgent::new(
        &ctx,
        session.id().clone(),
        dsh_agent::AgentOptions {
            provider: Some("fixture".into()),
            model: Some("synthetic".into()),
            ..Default::default()
        },
        session,
    )
    .unwrap();
    agents.enter(agent.clone(), None).unwrap();
    let message = |text: &str| {
        dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text { text: text.into() }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )
    };
    agent.followup(message("A task"));
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    agent.send(
        message("retained queue"),
        dsh_agent::InboxTarget::NextTurn,
        false,
    );
    let attempted = Arc::new(AtomicUsize::new(0));
    let observed = attempted.clone();
    let fault = ctx
        .on(
            "session/flush",
            Arc::new(move |_, _| {
                let observed = observed.clone();
                Box::pin(async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    panic!("synthetic account pause durability failure");
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    let impact = auth
        .handle(
            "logout-impact",
            &json!({"provider":p.id,"accountScope":a.account_scope}),
        )
        .await
        .unwrap();
    assert_eq!(impact["taskCount"], 1);
    let error = auth.handle("logout", &impact).await.unwrap_err();
    assert!(
        error.contains("持久化未确认") && error.contains("没有自动激活其他账号"),
        "{error}"
    );
    assert!(
        attempted.load(Ordering::SeqCst) > 0,
        "the real Session flush boundary must fail"
    );
    assert!(auth.session(p.id).await.unwrap().is_none());
    assert_eq!(auth.saved_sessions(p.id).await.unwrap().len(), 1);
    assert_eq!(
        auth.saved_sessions(p.id).await.unwrap()[0].account_scope,
        b.account_scope
    );
    assert!(captured.is_revoked());
    tokio::time::timeout(Duration::from_secs(2), agent.when_idle())
        .await
        .unwrap();
    assert_eq!(agent.inbox().next_turn().len(), 1);
    assert_eq!(agent.status(), dsh_agent::AgentStatus::Idle);
    fault().await;
    auth.credentials.drain().await;
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}
