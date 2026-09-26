use super::*;
use dsh_session::{
    SessionEvent, SessionHeader, SessionId, SessionLogOffset, SessionPreparation, SessionStore,
};
use dsh_session_persistence::{
    SessionInspection, SessionLocation, SessionPersistenceApi, SessionPersistenceSnapshot,
    SessionReadFromResult,
};
use dsh_session_persistence_jsonl::{
    JsonlConfig, JsonlSessionPersistence, compress_zstd_frame, session_dir,
};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Any reintroduced whole-history preflight fails before the restore can
/// accidentally pass through the otherwise-correct bounded preparation.
struct BoundedResumeOnly {
    inner: Arc<JsonlSessionPersistence>,
    prepares: AtomicUsize,
    snapshots: AtomicUsize,
    authority: Mutex<Option<SessionHeader>>,
}

#[async_trait::async_trait]
impl SessionPersistenceApi for BoundedResumeOnly {
    fn locate(&self, meta: &SessionHeader) -> Option<SessionLocation> {
        self.inner.locate(meta)
    }
    fn supports_raw_artifacts(&self) -> bool {
        self.inner.supports_raw_artifacts()
    }
    async fn create(
        &self,
        meta: SessionHeader,
        inherited: Option<SessionLogOffset>,
    ) -> Result<(), String> {
        self.inner.create(meta, inherited).await
    }
    async fn append(&self, id: &SessionId, events: &[SessionEvent]) -> Result<(), String> {
        self.inner.append(id, events).await
    }
    async fn load(&self, _: &SessionId) -> Result<SessionInspection, String> {
        panic!("cold routing must not load complete history")
    }
    async fn inspect(&self, _: &SessionId) -> Result<SessionInspection, String> {
        panic!("cold routing must not inspect complete history")
    }
    async fn read_from(&self, _: &SessionId, _: u64) -> Result<SessionReadFromResult, String> {
        panic!("cold routing must not materialize a suffix")
    }
    async fn prepare(&self, id: &SessionId) -> Result<SessionPreparation, String> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        self.inner.prepare(id).await
    }
    async fn read_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionPersistenceSnapshot>, String> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        let mut snapshot = self.inner.read_snapshot(id).await?;
        if let (Some(snapshot), Some(authority)) = (&mut snapshot, &*self.authority.lock()) {
            snapshot.header = authority.clone();
        }
        Ok(snapshot)
    }
    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        self.inner.list().await
    }
    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String> {
        self.inner.list_snapshots().await
    }
    fn ctx(&self) -> &Context {
        self.inner.ctx()
    }
}

struct Fixture {
    ctx: Context,
    sessions: Arc<SessionStore>,
    agents: Arc<dsh_agent::AgentRegistry>,
    persistence: Arc<BoundedResumeOnly>,
    root: std::path::PathBuf,
    original: std::path::PathBuf,
    original_bytes: Vec<u8>,
    id: SessionId,
}

impl Fixture {
    async fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("dsh-resolver-archive-{}", uuid::Uuid::new_v4()));
        let data_root = root.join("data");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = dsh_session::session_id("bounded-resolver");
        let directory = session_dir(
            &data_root.to_string_lossy(),
            Some(&cwd.to_string_lossy()),
            &id,
        );
        std::fs::create_dir_all(&directory).unwrap();
        let original = directory.join("session.jsonl.zstd");
        let mut file = std::fs::File::create(&original).unwrap();
        let header = serde_json::json!({"type":"session","version":3,"id":id,"createdAt":1,
            "cwd":cwd.to_string_lossy(),"delegationDepth":0,"agentPreset":"created-preset"});
        file.write_all(&compress_zstd_frame(format!("{header}\n").as_bytes()).unwrap())
            .unwrap();
        let mut batch = Vec::new();
        for seq in 0..4099_u64 {
            let (kind, data) = match seq {
                0 => (
                    "agent-preset/selected",
                    serde_json::json!({"agentPreset":"earlier-preset"}),
                ),
                1 => (
                    "model/selection",
                    serde_json::json!({"executionMode":"standard","provider":"stored-provider","model":"stored-model"}),
                ),
                4098 => (
                    "agent-preset/selected",
                    serde_json::json!({"agentPreset":"selected-preset"}),
                ),
                _ => (
                    "tools/discovery",
                    serde_json::json!({"index":seq,"payload":"x".repeat(1024)}),
                ),
            };
            let event = serde_json::json!({"seq":seq,"time":seq+1,"type":kind,"data":data});
            serde_json::to_writer(&mut batch, &event).unwrap();
            batch.push(b'\n');
            if seq % 32 == 31 || seq == 4098 {
                file.write_all(&compress_zstd_frame(&batch).unwrap())
                    .unwrap();
                batch.clear();
            }
        }
        drop(file);
        let original_bytes = std::fs::read(&original).unwrap();
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let sessions = SessionStore::install(&ctx);
        let agents = dsh_agent::AgentRegistry::install(&ctx);
        dsh_agent_loop::AgentLoop::install(&ctx, Default::default()).unwrap();
        let inner = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: data_root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        )
        .unwrap();
        let persistence = Arc::new(BoundedResumeOnly {
            inner,
            prepares: AtomicUsize::new(0),
            snapshots: AtomicUsize::new(0),
            authority: Mutex::new(None),
        });
        let erased: Arc<dyn SessionPersistenceApi> = persistence.clone();
        ctx.set("sessionPersistence", cordis::arc(erased)).unwrap();
        Self {
            ctx,
            sessions,
            agents,
            persistence,
            root,
            original,
            original_bytes,
            id,
        }
    }
    async fn close(self) {
        self.ctx.fiber.dispose().await;
        assert_eq!(
            std::fs::read(&self.original).unwrap(),
            self.original_bytes,
            "V3 source must remain byte-identical"
        );
        drop(self.persistence);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn actual_api_resolver_prepares_one_archive_and_restores_preset_model_and_every_row() {
    let fixture = Fixture::new().await;
    let service = ApiProxyService::install(&fixture.ctx, ApiProxyDefaults::default());
    let (first, second) = tokio::join!(
        service.resolver.resolve(&fixture.id),
        service.resolver.resolve(&fixture.id)
    );
    let crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) = first else {
        panic!("cold resume failed: {first:?}")
    };
    let crate::agent_lookup::ApiRemoteAgentResult::Agent(same) = second else {
        panic!("shared cold resume failed: {second:?}")
    };
    assert!(Arc::ptr_eq(&agent, &same));
    drop(same);
    assert_eq!(fixture.persistence.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.persistence.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(agent.session().header().version, 4);
    assert_eq!(agent.session().first_live_seq().get(), 4099);
    assert_eq!(
        persisted_agent_preset(agent.session()).unwrap().as_deref(),
        Some("selected-preset")
    );
    let selection_name = dsh_agent::model_selection::model_selection_service_name(agent.ctx());
    let selection = agent
        .ctx()
        .get_typed::<Arc<Mutex<ModelSelectionRef>>>(&selection_name, false)
        .unwrap();
    assert_eq!(
        selection.lock().resolved_current().unwrap().model,
        "stored-model"
    );
    let mut rows = 0;
    agent
        .session()
        .visit_events(0, Some(4099), |event| {
            assert_eq!(event.seq.get(), rows);
            if event.type_ == "tools/discovery" {
                assert_eq!(event.data["index"].as_u64(), Some(rows));
                assert_eq!(event.data["payload"].as_str().unwrap(), "x".repeat(1024));
            }
            rows += 1;
            Ok(true)
        })
        .unwrap();
    assert_eq!(rows, 4099);
    service.retire_idle_agent_for_test(agent.clone()).await;
    drop(agent);
    drop(selection);
    drop(service);
    fixture.close().await;
}

#[tokio::test]
async fn changed_cold_authority_is_rejected_before_setup_or_publication() {
    let fixture = Fixture::new().await;
    let mut authority = fixture
        .persistence
        .inner
        .read_snapshot(&fixture.id)
        .await
        .unwrap()
        .unwrap()
        .header;
    authority.cwd = Some(
        fixture
            .root
            .join("different-workspace")
            .to_string_lossy()
            .into_owned(),
    );
    *fixture.persistence.authority.lock() = Some(authority);
    let setup_calls = Arc::new(AtomicUsize::new(0));
    let counted = setup_calls.clone();
    let resolver = crate::agent_lookup::AgentResolver::new(
        &fixture.ctx,
        crate::agent_lookup::ApiRemoteAgentOptions {
            agent_options: Arc::new(Default::default),
            retain_handle: Arc::new(|_| panic!("changed authority cannot publish")),
            setup: Some(Arc::new(move |_, _| {
                let counted = counted.clone();
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
            })),
        },
    );
    assert!(matches!(
        resolver.resolve(&fixture.id).await,
        crate::agent_lookup::ApiRemoteAgentResult::Error(_)
    ));
    assert_eq!(setup_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.sessions.get(&fixture.id).is_none());
    assert!(fixture.agents.get(&fixture.id).is_none());
    drop(
        fixture
            .persistence
            .inner
            .prepare(&fixture.id)
            .await
            .unwrap(),
    );
    drop(resolver);
    fixture.close().await;
}

#[tokio::test]
async fn mismatched_snapshot_identity_is_rejected_without_preparing() {
    let fixture = Fixture::new().await;
    let mut authority = fixture
        .persistence
        .inner
        .read_snapshot(&fixture.id)
        .await
        .unwrap()
        .unwrap()
        .header;
    authority.id = dsh_session::session_id("another-session");
    *fixture.persistence.authority.lock() = Some(authority);
    let resolver = crate::agent_lookup::AgentResolver::new(
        &fixture.ctx,
        crate::agent_lookup::ApiRemoteAgentOptions {
            agent_options: Arc::new(Default::default),
            retain_handle: Arc::new(|_| panic!("wrong identity cannot publish")),
            setup: None,
        },
    );
    let result = resolver.resolve(&fixture.id).await;
    assert!(
        matches!(
            result,
            crate::agent_lookup::ApiRemoteAgentResult::Error(RpcError::Internal(_))
        ),
        "unexpected result: {result:?}"
    );
    assert_eq!(fixture.persistence.prepares.load(Ordering::SeqCst), 0);
    assert!(fixture.sessions.get(&fixture.id).is_none());
    assert!(fixture.agents.get(&fixture.id).is_none());
    drop(resolver);
    fixture.close().await;
}

#[tokio::test]
async fn changed_lineage_seed_or_preset_cannot_publish_prepared_history() {
    for change in ["parent", "seed", "depth", "preset"] {
        let fixture = Fixture::new().await;
        let mut authority = fixture
            .persistence
            .inner
            .read_snapshot(&fixture.id)
            .await
            .unwrap()
            .unwrap()
            .header;
        match change {
            "parent" => {
                authority.parent_session = Some(dsh_session::session_id("different-parent"))
            }
            "seed" => authority.is_seeded = true,
            "depth" => authority.delegation_depth = Some(3),
            "preset" => authority.agent_preset = Some("different-preset".into()),
            _ => unreachable!(),
        }
        *fixture.persistence.authority.lock() = Some(authority);
        let resolver = crate::agent_lookup::AgentResolver::new(
            &fixture.ctx,
            crate::agent_lookup::ApiRemoteAgentOptions {
                agent_options: Arc::new(Default::default),
                retain_handle: Arc::new(|_| panic!("changed authority cannot publish")),
                setup: None,
            },
        );
        let result = resolver.resolve(&fixture.id).await;
        assert!(
            matches!(
                result,
                crate::agent_lookup::ApiRemoteAgentResult::Error(RpcError::Internal(_))
            ),
            "{change}: {result:?}"
        );
        assert_eq!(
            fixture.persistence.prepares.load(Ordering::SeqCst),
            1,
            "{change}"
        );
        assert!(fixture.sessions.get(&fixture.id).is_none(), "{change}");
        assert!(fixture.agents.get(&fixture.id).is_none(), "{change}");
        // An error must also release the unpublished preparation's writer lease.
        drop(
            fixture
                .persistence
                .inner
                .prepare(&fixture.id)
                .await
                .unwrap(),
        );
        drop(resolver);
        fixture.close().await;
    }
}

#[tokio::test]
async fn last_cancelled_async_setup_releases_archive_and_writer_and_can_retry() {
    let fixture = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let block = Arc::new(AtomicBool::new(true));
    let weak_agent = Arc::new(Mutex::new(None));
    let handles = Arc::new(Mutex::new(Vec::<dsh_agent::AgentHandle>::new()));
    let retained = handles.clone();
    let setup_entered = entered.clone();
    let setup_block = block.clone();
    let observed = weak_agent.clone();
    let resolver = crate::agent_lookup::AgentResolver::new(
        &fixture.ctx,
        crate::agent_lookup::ApiRemoteAgentOptions {
            agent_options: Arc::new(Default::default),
            retain_handle: Arc::new(move |handle| {
                let agent = handle.agent.clone();
                retained.lock().push(handle);
                agent
            }),
            setup: Some(Arc::new(move |_, agent| {
                let entered = setup_entered.clone();
                let block = setup_block.clone();
                let observed = observed.clone();
                Box::pin(async move {
                    *observed.lock() = Some(Arc::downgrade(&agent));
                    entered.notify_one();
                    if block.load(Ordering::SeqCst) {
                        std::future::pending::<()>().await;
                    }
                    Ok(None)
                })
            })),
        },
    );
    let active = {
        let resolver = resolver.clone();
        let id = fixture.id.clone();
        tokio::spawn(async move { resolver.resolve(&id).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), entered.notified())
        .await
        .unwrap();
    active.abort();
    assert!(active.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while weak_agent
            .lock()
            .as_ref()
            .is_some_and(|weak| weak.upgrade().is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("abandoned setup must release its prepared Agent");
    assert!(fixture.sessions.get(&fixture.id).is_none());
    assert!(fixture.agents.get(&fixture.id).is_none());
    block.store(false, Ordering::SeqCst);
    let result = resolver.resolve(&fixture.id).await;
    assert!(
        matches!(result, crate::agent_lookup::ApiRemoteAgentResult::Agent(_)),
        "retry failed: {result:?}"
    );
    assert_eq!(fixture.persistence.prepares.load(Ordering::SeqCst), 2);
    drop(result);
    let handle = handles.lock().pop().unwrap();
    handle.dispose.await;
    drop(handle.agent);
    drop(resolver);
    fixture.close().await;
}

#[tokio::test]
async fn cancelling_one_of_two_waiters_keeps_the_shared_preparation_alive() {
    let fixture = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let weak_agent = Arc::new(Mutex::new(None));
    let handles = Arc::new(Mutex::new(Vec::<dsh_agent::AgentHandle>::new()));
    let retained = handles.clone();
    let setup_entered = entered.clone();
    let setup_release = release.clone();
    let observed = weak_agent.clone();
    let resolver = crate::agent_lookup::AgentResolver::new(
        &fixture.ctx,
        crate::agent_lookup::ApiRemoteAgentOptions {
            agent_options: Arc::new(Default::default),
            retain_handle: Arc::new(move |handle| {
                let agent = handle.agent.clone();
                retained.lock().push(handle);
                agent
            }),
            setup: Some(Arc::new(move |_, agent| {
                let entered = setup_entered.clone();
                let release = setup_release.clone();
                let observed = observed.clone();
                Box::pin(async move {
                    *observed.lock() = Some(Arc::downgrade(&agent));
                    entered.notify_one();
                    release.notified().await;
                    Ok(None)
                })
            })),
        },
    );
    let active = {
        let resolver = resolver.clone();
        let id = fixture.id.clone();
        tokio::spawn(async move { resolver.resolve(&id).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), entered.notified())
        .await
        .unwrap();
    let remaining = {
        let resolver = resolver.clone();
        let id = fixture.id.clone();
        async move { resolver.resolve(&id).await }
    };
    tokio::pin!(remaining);
    // Polling enrols this second caller before the original caller cancels.
    assert!(futures::poll!(&mut remaining).is_pending());
    active.abort();
    assert!(active.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.persistence.prepares.load(Ordering::SeqCst), 1);
    assert!(weak_agent.lock().as_ref().unwrap().upgrade().is_some());
    assert!(fixture.agents.get(&fixture.id).is_none());
    release.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), &mut remaining)
        .await
        .unwrap();
    assert!(
        matches!(result, crate::agent_lookup::ApiRemoteAgentResult::Agent(_)),
        "remaining waiter failed: {result:?}"
    );
    assert_eq!(fixture.persistence.prepares.load(Ordering::SeqCst), 1);
    drop(result);
    let handle = handles.lock().pop().unwrap();
    handle.dispose.await;
    drop(handle.agent);
    drop(remaining);
    drop(resolver);
    fixture.close().await;
}
