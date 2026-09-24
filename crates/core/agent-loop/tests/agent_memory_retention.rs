use cordis::Context;
use dsh_agent::{Agent, AgentFactory, AgentOptions, AgentRegistry, CreateAgentOptions};
use dsh_agent_loop::{AgentLoop, ReactLoopAgent};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmFailure, LlmRuntime,
    StreamChunk,
};
use dsh_session::{SessionStore, session_id};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct Adapter {
    calls: AtomicUsize,
    fail: bool,
}
impl LlmAdapter for Adapter {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
            reason: if self.fail {
                FinishReason::Error {
                    failure: LlmFailure {
                        message: "controlled failure".into(),
                        code: "TRANSPORT".into(),
                        status: None,
                        offload_images: None,
                        provider_retry_after_ms: None,
                        request_id: None,
                    },
                }
            } else {
                FinishReason::Stop
            },
            replay_state: None,
        }]))
    }
}
fn fixture(fail: bool) -> (Context, Arc<SessionStore>, Arc<AgentRegistry>, Arc<Adapter>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let llm = LlmRuntime::install(&ctx);
    dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    let sessions = SessionStore::install(&ctx);
    let agents = AgentRegistry::install(&ctx);
    let adapter = Arc::new(Adapter {
        calls: AtomicUsize::new(0),
        fail,
    });
    llm.register_adapter(&ctx, vec!["memory-test".into()], adapter.clone())
        .unwrap();
    (ctx, sessions, agents, adapter)
}
fn options() -> AgentOptions {
    AgentOptions {
        provider: Some("memory-test".into()),
        model: Some("fixture".into()),
        ..Default::default()
    }
}
async fn response(agent: &dyn Agent, adapter: &Adapter) {
    let before = adapter.calls.load(Ordering::SeqCst);
    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "memory regression".into(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while adapter.calls.load(Ordering::SeqCst) == before {
            tokio::task::yield_now().await;
        }
        agent.when_idle().await;
    })
    .await
    .expect("driver must execute and settle");
    assert!(
        agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "turn/end")
    );
}
async fn released<T: ?Sized>(weak: &Weak<T>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired real agent must release its final strong reference");
}
#[tokio::test]
async fn direct_agents_release_after_normal_and_failed_dispatch() {
    for fail in [false, true] {
        let (ctx, sessions, _, adapter) = fixture(fail);
        for talk in [false, true] {
            let session = sessions
                .prepare(Some(session_id(format!("direct-{talk}"))), None)
                .unwrap();
            let agent =
                ReactLoopAgent::new(&ctx, session.id().clone(), options(), session).unwrap();
            let weak = Arc::downgrade(&agent);
            if talk {
                response(agent.as_ref(), &adapter).await;
            }
            (agent.scope().dispose)().await;
            drop(agent);
            released(&weak).await;
        }
        ctx.fiber.dispose().await;
    }
}
#[tokio::test]
async fn hundred_generations_release_while_factory_remains_alive() {
    let (ctx, sessions, agents, adapter) = fixture(false);
    let factory = AgentLoop::install(&ctx, Default::default()).unwrap();
    let mut retired = Vec::new();
    for generation in 0..100 {
        let handle = factory
            .create_agent(
                &ctx,
                CreateAgentOptions {
                    session_id: Some(session_id("same-id")),
                    agent_options: Some(options()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        handle
            .agent
            .session()
            .append(
                "tools/discovery",
                serde_json::json!({"text":"x".repeat((generation + 1) * 1024)}),
                None,
            )
            .unwrap();
        response(handle.agent.as_ref(), &adapter).await;
        retired.push(Arc::downgrade(&handle.agent));
        // Both the public handle and the retirement service join one teardown.
        let ((), retired_ok) = tokio::join!(handle.dispose, factory.retire(handle.agent.clone()));
        assert!(retired_ok.unwrap());
        drop(handle.agent);
        released(retired.last().unwrap()).await;
        assert!(agents.list().is_empty());
        assert!(sessions.list().is_empty());
        assert!(retired.iter().all(|agent| agent.upgrade().is_none()));
    }
    ctx.fiber.dispose().await;
}
#[tokio::test]
async fn setup_error_and_cancelled_creation_roll_back_ownership() {
    for cancel in [false, true] {
        let (ctx, sessions, agents, _) = fixture(false);
        let factory = AgentLoop::install(&ctx, Default::default()).unwrap();
        let seen = Arc::new(parking_lot::Mutex::new(None));
        let entered = Arc::new(tokio::sync::Notify::new());
        let capture = seen.clone();
        let notify = entered.clone();
        let options = CreateAgentOptions {
            setup: Some(Arc::new(move |_, agent| {
                *capture.lock() = Some(Arc::downgrade(&agent));
                notify.notify_one();
                Box::pin(async move {
                    if cancel {
                        futures::future::pending::<()>().await;
                    }
                    Err("setup rejected".into())
                })
            })),
            ..Default::default()
        };
        let owner = ctx.clone();
        let create = factory.clone();
        let task = tokio::spawn(async move { create.create_agent(&owner, options).await });
        entered.notified().await;
        if cancel {
            task.abort();
            let _ = task.await;
        } else {
            assert!(matches!(task.await.unwrap(), Err(error) if error == "setup rejected"));
        }
        let weak = seen.lock().take().unwrap();
        released(&weak).await;
        assert!(agents.list().is_empty());
        assert!(sessions.list().is_empty());
        ctx.fiber.dispose().await;
    }
}

#[tokio::test]
async fn fifty_real_persistence_resumes_release_old_history_generations() {
    use dsh_session_persistence::SessionPersistenceApi;
    use dsh_session_persistence_jsonl::{JsonlConfig, JsonlSessionPersistence};
    let (ctx, _, _, adapter) = fixture(false);
    let root = std::env::temp_dir().join(format!("agent-retention-{}", uuid::Uuid::new_v4()));
    let persistence = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            prepared_session_cache_size: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let factory = AgentLoop::install(&ctx, Default::default()).unwrap();
    let id = session_id("persisted-history");
    let mut handle = factory
        .create_agent(
            &ctx,
            CreateAgentOptions {
                session_id: Some(id.clone()),
                agent_options: Some(options()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for generation in 0..50 {
        assert_eq!(
            handle
                .agent
                .session()
                .events()
                .iter()
                .filter(|e| e.type_ == "tools/discovery")
                .count(),
            generation
        );
        handle
            .agent
            .session()
            .append(
                "tools/discovery",
                serde_json::json!({"text":"x".repeat(16 * 1024)}),
                None,
            )
            .unwrap();
        response(handle.agent.as_ref(), &adapter).await;
        let weak = Arc::downgrade(&handle.agent);
        handle.dispose.await;
        drop(handle.agent);
        released(&weak).await;
        handle = factory
            .resume(
                &ctx,
                dsh_agent::ResumeAgentOptions {
                    resume_session_id: Some(id.clone()),
                    agent_options: Some(options()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(
        handle
            .agent
            .session()
            .events()
            .iter()
            .filter(|e| e.type_ == "tools/discovery")
            .count(),
        50
    );
    handle.dispose.await;
    drop(handle.agent);
    assert_eq!(
        persistence
            .inspect(&id)
            .await
            .unwrap()
            .events
            .iter()
            .filter(|e| e.type_ == "tools/discovery")
            .count(),
        50
    );
    ctx.fiber.dispose().await;
    drop(persistence);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn competing_factories_reserve_storage_before_setup_and_release_failed_resumes() {
    use dsh_session_persistence::SessionPersistenceApi;
    use dsh_session_persistence_jsonl::{JsonlConfig, JsonlSessionPersistence};
    let root = std::env::temp_dir().join(format!("agent-writer-owner-{}", uuid::Uuid::new_v4()));
    let (a_ctx, _, _, a_adapter) = fixture(false);
    let (b_ctx, b_sessions, b_agents, b_adapter) = fixture(false);
    let config = || JsonlConfig { root: root.to_string_lossy().into_owned(), ..Default::default() };
    let a_persistence = JsonlSessionPersistence::install(&a_ctx, config()).unwrap();
    let _b_persistence = JsonlSessionPersistence::install(&b_ctx, config()).unwrap();
    let a = AgentLoop::install(&a_ctx, Default::default()).unwrap();
    let b = AgentLoop::install(&b_ctx, Default::default()).unwrap();
    let id = session_id("single-writer-agent");
    let first = a.create_agent(&a_ctx, CreateAgentOptions {
        session_id: Some(id.clone()), agent_options: Some(options()), ..Default::default()
    }).await.unwrap();
    response(first.agent.as_ref(), &a_adapter).await;
    let setup_calls = Arc::new(AtomicUsize::new(0));
    let counting_setup: dsh_agent::AgentSetup = Arc::new({
        let setup_calls = setup_calls.clone();
        move |_, _| { setup_calls.fetch_add(1, Ordering::SeqCst); Box::pin(async { Ok(None) }) }
    });
    let blocked = b.resume(&b_ctx, dsh_agent::ResumeAgentOptions {
        resume_session_id: Some(id.clone()), agent_options: Some(options()), setup: Some(counting_setup.clone())
    }).await.unwrap_err();
    assert!(blocked.contains("SESSION_IN_USE"), "{blocked}");
    let blocked = b.create_agent(&b_ctx, CreateAgentOptions {
        session_id: Some(id.clone()), agent_options: Some(options()), setup: Some(counting_setup), ..Default::default()
    }).await.unwrap_err();
    assert!(blocked.contains("SESSION_IN_USE"), "{blocked}");
    assert_eq!(setup_calls.load(Ordering::SeqCst), 0);
    assert_eq!(b_adapter.calls.load(Ordering::SeqCst), 0);
    assert!(b_sessions.list().is_empty()); assert!(b_agents.list().is_empty());
    first.dispose.await; drop(first.agent);
    a_persistence.inspect(&id).await.unwrap();
    let failed = b.resume(&b_ctx, dsh_agent::ResumeAgentOptions {
        resume_session_id: Some(id.clone()), agent_options: Some(options()),
        setup: Some(Arc::new(|_, _| Box::pin(async { Err("controlled setup failure".into()) }))),
    }).await.unwrap_err();
    assert_eq!(failed, "controlled setup failure");
    let recovered = a.resume(&a_ctx, dsh_agent::ResumeAgentOptions {
        resume_session_id: Some(id.clone()), agent_options: Some(options()), ..Default::default()
    }).await.unwrap();
    response(recovered.agent.as_ref(), &a_adapter).await;
    recovered.dispose.await; drop(recovered.agent);
    for ctx in [&a_ctx, &b_ctx] {
        for dispose in ctx.fiber.disposables.clear() { dispose().await; }
    }
    std::fs::remove_dir_all(root).unwrap();
}
