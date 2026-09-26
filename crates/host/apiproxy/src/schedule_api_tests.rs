use super::*;
use dsh_schedule::host_service::{Config, ScheduleService};
use dsh_session_persistence_jsonl::{
    JsonlConfig, JsonlSessionPersistence, compress_zstd_frame, session_dir,
};
use serde_json::{Value, json};

struct Fixture {
    ctx: Context,
    api: Arc<ApiProxyService>,
    schedule: Arc<ScheduleService>,
    root: std::path::PathBuf,
    source: std::path::PathBuf,
    source_bytes: Vec<u8>,
}

#[tokio::test]
async fn schedule_delivery_requires_a_participating_persistence_barrier() {
    let (api, agent, detach) =
        super::idle_retirement_tests::control_admission_fixture("schedule-no-persistence").await;
    let lease = api
        .schedule_session_controller()
        .acquire(agent.id().as_str(), true)
        .await
        .unwrap();
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "Check durable admission".into(),
        }],
        dsh_llm::MessageSource::Plugin {
            plugin: "schedule".into(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    let error = lease.deliver(message).await.unwrap_err();
    assert_eq!(error.code, "persistence_failed");
    drop(lease);
    detach().await;
}

#[tokio::test]
async fn schedule_http_preserves_null_edits_number_semantics_and_cas_validation_order() {
    let fixture = Fixture::new(None).await;
    let record = fixture.create().await;
    for (field, reason) in [
        ("title", "invalid_prompt"),
        ("prompt", "invalid_prompt"),
        ("change", "invalid_rule"),
    ] {
        let mut request =
            json!({"sessionId":"cold-reminder-owner","id":record["id"],"expected":record});
        request[field] = Value::Null;
        let rejected = fixture.rpc("schedule.update", request).await;
        assert_eq!(rejected["error"]["details"]["reason"], reason, "{rejected}");
    }
    let mut stale_record = record.clone();
    stale_record["title"] = json!("Previously observed title");
    let conflict = fixture.rpc("schedule.update", json!({"sessionId":"cold-reminder-owner","id":record["id"],"expected":stale_record,"title":null,"change":null})).await;
    assert_eq!(conflict["value"]["code"], "schedule_conflict", "{conflict}");
    let missing = fixture
        .rpc(
            "schedule.update",
            json!({"sessionId":"cold-reminder-owner","id":"missing","expected":null,"title":null}),
        )
        .await;
    assert_eq!(missing["value"]["code"], "schedule_not_found", "{missing}");
    let unknown = fixture.rpc("schedule.update", json!({"sessionId":"cold-reminder-owner","id":record["id"],"expected":record,"unknown":true})).await;
    assert_eq!(unknown["error"]["details"]["reason"], "invalid_rule");
    let unknown_create = fixture.rpc("schedule.create", json!({"sessionId":"cold-reminder-owner","title":"Unknown field","prompt":"Keep strictness","after_seconds":60,"unknown":true})).await;
    assert_eq!(unknown_create["error"]["details"]["reason"], "invalid_rule");
    let null_selector = fixture.rpc("schedule.create", json!({"sessionId":"cold-reminder-owner","title":"Null rule","prompt":"Reject explicit null","after_seconds":null,"every_seconds":60})).await;
    assert_eq!(
        null_selector["error"]["details"]["reason"],
        "invalid_selector"
    );
    for selector in ["after_seconds", "every_seconds"] {
        let mut request = json!({"sessionId":"cold-reminder-owner","title":"Integer JSON number","prompt":"Accept integral JSON numeric values"});
        request[selector] = json!(60.0);
        let created = fixture.rpc("schedule.create", request).await;
        assert_eq!(created["ok"], true, "{created}");
    }
    let page = fixture
        .rpc(
            "schedule.history",
            json!({"sessionId":"cold-reminder-owner","id":record["id"],"limit":1.0}),
        )
        .await;
    assert_eq!(page["ok"], true, "{page}");
    let null_cursor = fixture
        .rpc(
            "schedule.history",
            json!({"sessionId":"cold-reminder-owner","id":record["id"],"limit":1.0,"before":null}),
        )
        .await;
    assert_eq!(null_cursor["error"]["details"]["reason"], "invalid_rule");
    fixture.close(false).await;
}

impl Fixture {
    async fn new(origin: Option<&str>) -> Self {
        Self::with_workspace(origin, true).await
    }

    async fn with_workspace(origin: Option<&str>, bound: bool) -> Self {
        let root = std::env::temp_dir().join(format!("schedule-api-{}", uuid::Uuid::new_v4()));
        let id = dsh_session::session_id("cold-reminder-owner");
        let cwd = bound.then(|| root.join("workspace"));
        if let Some(cwd) = &cwd {
            std::fs::create_dir_all(cwd).unwrap();
        }
        let cwd = cwd.map(|path| path.to_string_lossy().into_owned());
        let directory = session_dir(&root.to_string_lossy(), cwd.as_deref(), &id);
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("session.jsonl.zstd");
        let mut header =
            json!({"type":"session","version":3,"id":id,"createdAt":1,"delegationDepth":0});
        if let Some(origin) = origin {
            header["origin"] = json!(origin);
        }
        if let Some(cwd) = &cwd {
            header["cwd"] = json!(cwd);
        }
        let source_bytes = compress_zstd_frame(format!("{header}\n").as_bytes()).unwrap();
        std::fs::write(&source, &source_bytes).unwrap();
        let ctx = Context::root();
        let sessions = dsh_session::SessionStore::install(&ctx);
        dsh_agent::AgentRegistry::install(&ctx);
        // No Agent factory is installed: a supposedly cold management operation
        // fails if it accidentally tries to restore the owner.
        let persistence = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        )
        .unwrap();
        let storage = dsh_storage::Storage::install(&ctx);
        let _backend = storage
            .backend
            .register(
                "memory",
                Arc::new(dsh_storage_test_support::MemoryStorageBackend::new(
                    Arc::new(dsh_storage_test_support::MemoryMediaPool::new()),
                )),
            )
            .unwrap();
        let domains = dsh_storage_domain::DomainFacility::install(
            &ctx,
            dsh_storage_domain::DomainFacilityConfig {
                backend: "memory".into(),
                routes: Default::default(),
            },
        )
        .unwrap();
        let deleting = persistence.clone();
        use dsh_session_persistence::SessionPersistenceApi;
        dsh_workspace::WorkspaceRegistry::install(
            &ctx,
            &domains,
            persistence,
            Some(Arc::new(dsh_workspace::StoreLiveSessions(sessions))),
            Arc::new(move |id| {
                let deleting = deleting.clone();
                let id = id.clone();
                Box::pin(async move { deleting.delete(&id).await })
            }),
        )
        .unwrap();
        let api = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let schedule = ScheduleService::install(&ctx, api.schedule_session_controller());
        schedule.enable(Config::default()).await.unwrap();
        Self {
            ctx,
            api,
            schedule,
            root,
            source,
            source_bytes,
        }
    }

    async fn rpc(&self, method: &str, payload: Value) -> Value {
        let response = crate::fetch::handler::to_fetch_handler(self.api.clone()).handle(crate::fetch::handler::CarrierRequest {
            method:http::Method::POST, path:format!("/api/{method}"), query:vec![], headers:vec![("content-type".into(),"application/json".into())],
            body:Some(json!({"type":"client-request","rpcId":uuid::Uuid::new_v4().to_string(),"method":method,"payload":payload}).to_string().into_bytes()),
        }).await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary response required")
        };
        serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
    }

    async fn create(&self) -> Value {
        let result = self.rpc("schedule.create", json!({"sessionId":"cold-reminder-owner","title":"Check build","prompt":"Review the completed build","after_seconds":86400})).await;
        assert_eq!(result["ok"], true, "{result}");
        result["value"].clone()
    }

    async fn close(self, deleted: bool) {
        self.schedule.shutdown().await;
        self.ctx.fiber.dispose().await;
        if deleted {
            assert!(!self.source.exists());
        } else {
            assert_eq!(std::fs::read(&self.source).unwrap(), self.source_bytes);
        }
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn schedule_creation_rejects_a_session_without_cold_restore_authority_before_persistence() {
    let fixture = Fixture::with_workspace(None, false).await;
    let result = fixture.rpc("schedule.create", json!({"sessionId":"cold-reminder-owner","title":"Unrestorable owner","prompt":"Do not invent a workspace","after_seconds":60})).await;
    assert_eq!(result["error"]["code"], "schedule-rejected");
    assert_eq!(result["error"]["details"]["reason"], "session_unavailable");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("working directory")
    );
    assert!(fixture.schedule.catalog().await.unwrap().is_empty());
    fixture.close(false).await;
}

#[tokio::test]
async fn schedule_rpc_manages_cold_records_with_cas_and_never_restores_an_agent() {
    let fixture = Fixture::new(None).await;
    let record = fixture.create().await;
    let updated = fixture.rpc("schedule.update", json!({"sessionId":"cold-reminder-owner","id":record["id"],"expected":record,"title":"Changed title"})).await;
    assert_eq!(updated["ok"], true, "{updated}");
    assert_eq!(updated["value"]["updated"], true);
    let stale = fixture.rpc("schedule.update", json!({"sessionId":"cold-reminder-owner","id":record["id"],"expected":record,"title":"Stale title"})).await;
    assert_eq!(stale["value"]["code"], "schedule_conflict");
    let list = fixture
        .rpc("schedule.list", json!({"sessionId":"cold-reminder-owner"}))
        .await;
    assert_eq!(list["value"].as_array().unwrap().len(), 1);
    assert_eq!(list["value"][0]["title"], "Changed title");
    let history = fixture
        .rpc(
            "schedule.history",
            json!({"sessionId":"cold-reminder-owner","id":record["id"],"limit":1}),
        )
        .await;
    assert_eq!(history["value"]["records"], json!([]));
    let invalid = fixture
        .rpc(
            "schedule.history",
            json!({"sessionId":"cold-reminder-owner","id":record["id"],"limit":101}),
        )
        .await;
    assert_eq!(invalid["error"]["code"], "schedule-rejected");
    assert_eq!(invalid["error"]["details"]["reason"], "invalid_rule");
    fixture.schedule.disable().await;
    assert_eq!(
        fixture.rpc("schedule.catalog", json!({})).await["value"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture
            .rpc(
                "schedule.delete",
                json!({"sessionId":"cold-reminder-owner","id":record["id"]})
            )
            .await["value"]["deleted"],
        true
    );
    assert!(
        fixture
            .api
            .agents()
            .unwrap()
            .get(&dsh_session::session_id("cold-reminder-owner"))
            .is_none()
    );
    fixture.close(false).await;
}

#[tokio::test]
async fn schedule_archive_requires_explicit_stop_and_rejects_future_cold_delivery() {
    let fixture = Fixture::new(None).await;
    fixture.create().await;
    let denied = fixture
        .rpc(
            "workspace.archiveSession",
            json!({"sessionId":"cold-reminder-owner"}),
        )
        .await;
    assert_eq!(denied["error"]["code"], "agent-busy");
    assert_eq!(denied["error"]["details"]["reason"], "active-schedules");
    let archived = fixture
        .rpc(
            "workspace.archiveSession",
            json!({"sessionId":"cold-reminder-owner","stopSchedules":true}),
        )
        .await;
    assert_eq!(archived["ok"], true, "{archived}");
    assert!(fixture.schedule.catalog().await.unwrap().is_empty());
    let denied = fixture
        .api
        .schedule_session_controller()
        .acquire("cold-reminder-owner", true)
        .await;
    assert_eq!(denied.err().unwrap().code, "session_archived");
    assert_eq!(
        fixture
            .rpc(
                "workspace.unarchiveSession",
                json!({"sessionId":"cold-reminder-owner"})
            )
            .await["ok"],
        true
    );
    fixture.create().await;
    fixture.close(false).await;
}

#[tokio::test]
async fn schedule_permanent_deletion_purges_stored_tasks_before_removing_the_log() {
    let fixture = Fixture::new(None).await;
    fixture.create().await;
    // Reproduce a stored archived session with tasks from an older Host.
    fixture
        .api
        .workspace_registry()
        .unwrap()
        .archive_session(&dsh_session::session_id("cold-reminder-owner"))
        .await
        .unwrap();
    let removed = fixture
        .rpc(
            "workspace.deleteArchivedSession",
            json!({"sessionId":"cold-reminder-owner"}),
        )
        .await;
    assert_eq!(removed["ok"], true, "{removed}");
    assert!(fixture.schedule.catalog().await.unwrap().is_empty());
    assert!(
        fixture
            .api
            .schedule_session_controller()
            .acquire("cold-reminder-owner", false)
            .await
            .is_err()
    );
    fixture.close(true).await;
}

#[tokio::test]
async fn schedule_management_shares_control_admission_and_rejects_subagent_ownership() {
    let fixture = Fixture::new(None).await;
    let controller = fixture.api.schedule_session_controller();
    let lease = controller
        .acquire("cold-reminder-owner", false)
        .await
        .unwrap();
    assert!(
        fixture
            .api
            .try_resolve_control_agent("cold-reminder-owner")
            .await
            .unwrap()
            .is_none()
    );
    {
        let signal = AbortSignal::new();
        let pending = fixture.api.invoke("schedule.create", RpcRequest { rpc_id: crate::api::rpc::rpc_id("cancelled-create"), payload:json!({"sessionId":"cold-reminder-owner","title":"Cancelled","prompt":"Never saved","after_seconds":86400}) }, signal.clone());
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending)
                .await
                .is_err()
        );
        signal.abort();
        let cancelled = pending.await;
        assert_eq!(
            cancelled.result.error().unwrap().code(),
            crate::api::rpc::RpcErrorCode::Cancelled
        );
    }
    drop(lease);
    assert!(fixture.schedule.catalog().await.unwrap().is_empty());
    fixture.close(false).await;
    let child = Fixture::new(Some("subagent")).await;
    assert_eq!(
        child
            .api
            .schedule_session_controller()
            .acquire("cold-reminder-owner", false)
            .await
            .err()
            .unwrap()
            .code,
        "session_unauthorized"
    );
    child.close(false).await;
}
