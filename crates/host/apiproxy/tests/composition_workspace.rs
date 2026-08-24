//! Composition-layer `workspace.*` over the real fetch carrier with the
//! memory domain facility: create/list/rename/delete roundtrips and the
//! error vocabulary.

use std::sync::Arc;

use cordis::Context;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{SessionHeader, SessionId, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_storage::Storage;
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};
use dsh_workspace::{LiveSessionStore, SessionDeleteFn, WorkspaceRegistry};
use parking_lot::Mutex;

static WS_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn sid(id: &str) -> SessionId {
    session_id(id)
}

fn header(id: &str) -> SessionHeader {
    SessionHeader {
        version: 1,
        id: sid(id),
        created_at: 0,
        cwd: Some("D:\\proj".to_string()),
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}

/// Header-only session persistence fake.
struct FakePersistence {
    listed: Arc<Mutex<Vec<SessionHeader>>>,
}

#[async_trait::async_trait]
impl SessionPersistenceApi for FakePersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, _meta: SessionHeader) -> Result<(), String> {
        Ok(())
    }

    async fn append(
        &self,
        _id: &SessionId,
        _events: &[dsh_session::SessionEvent],
    ) -> Result<(), String> {
        Ok(())
    }

    async fn load(
        &self,
        _id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        Err("event bodies must not be loaded".to_string())
    }

    async fn inspect(
        &self,
        _id: &SessionId,
    ) -> Result<dsh_session_persistence::SessionInspection, String> {
        Err("event bodies must not be inspected".to_string())
    }

    async fn read_from(
        &self,
        _id: &SessionId,
        _from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        Err("event bodies must not be read".to_string())
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self.listed.lock().clone())
    }

    async fn list_snapshots(
        &self,
    ) -> Result<Vec<dsh_session_persistence::SessionPersistenceSnapshot>, String> {
        Ok(Vec::new())
    }

    fn ctx(&self) -> &Context {
        unimplemented!("the fake is never asked for its context")
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    root: std::path::PathBuf,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let hub = Storage::install(&ctx);
        let pool = Arc::new(MemoryMediaPool::new());
        let backend = MemoryStorageBackend::with_shared_pool(pool.clone());
        hub.backend
            .register("memory", backend)
            .expect("register backend");
        let facility = DomainFacility::install(
            &ctx,
            DomainFacilityConfig {
                backend: "memory".to_string(),
                routes: Default::default(),
            },
        )
        .expect("domain facility");
        let persistence: Arc<dyn SessionPersistenceApi> = Arc::new(FakePersistence {
            listed: Arc::new(Mutex::new(vec![header("s1")])),
        });
        let noop_delete: SessionDeleteFn = Arc::new(|_id| Box::pin(async move { Ok(true) }));
        let _registry = WorkspaceRegistry::install(
            &ctx,
            &facility,
            persistence,
            None::<Arc<dyn LiveSessionStore>>,
            noop_delete,
        )
        .expect("registry");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let root = std::env::temp_dir().join(format!(
            "dsh-apiproxy-ws-{}-{}",
            std::process::id(),
            WS_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        Self {
            _ctx: ctx,
            handler,
            root,
        }
    }

    async fn post(&self, method: &str, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": method,
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: format!("/api/{method}"),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary answers are byte bodies");
        };
        serde_json::from_slice(&bytes).expect("json")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn create_lists_and_renames_a_workspace() {
    run(async {
        let harness = Harness::new();
        let path = harness.root.to_string_lossy().into_owned();
        let created = harness
            .post("workspace.create", serde_json::json!({ "path": path }))
            .await;
        assert_eq!(created["result"]["ok"], true, "{created}");
        let workspace = &created["result"]["value"]["workspace"];
        // Windows realpath normalization adds the verbatim prefix; compare
        // the canonical spelling.
        let stored_path = workspace["path"]
            .as_str()
            .expect("path")
            .strip_prefix(r"\\?\")
            .unwrap_or(workspace["path"].as_str().expect("path"));
        assert_eq!(stored_path, path);
        assert_eq!(created["result"]["value"]["created"], true);
        let workspace_id = workspace["workspaceId"].as_str().expect("id");

        // A second create reuses the same workspace.
        let again = harness
            .post("workspace.create", serde_json::json!({ "path": path }))
            .await;
        assert_eq!(again["result"]["ok"], true);
        assert_eq!(again["result"]["value"]["created"], false);
        assert_eq!(
            again["result"]["value"]["workspace"]["workspaceId"],
            workspace_id
        );

        let listed = harness.post("workspace.list", serde_json::json!({})).await;
        let items = listed["result"]["value"]["items"]
            .as_array()
            .expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["workspaceId"], workspace_id);

        let renamed = harness
            .post(
                "workspace.rename",
                serde_json::json!({ "workspaceId": workspace_id, "title": "My Project" }),
            )
            .await;
        assert_eq!(renamed["result"]["ok"], true, "{renamed}");
        assert_eq!(
            renamed["result"]["value"]["workspace"]["title"],
            "My Project"
        );
    });
}

#[test]
fn delete_removes_the_registration_and_unknown_ids_are_workspace_not_found() {
    run(async {
        let harness = Harness::new();
        let path = harness.root.to_string_lossy().into_owned();
        let created = harness
            .post("workspace.create", serde_json::json!({ "path": path }))
            .await;
        let workspace_id = created["result"]["value"]["workspace"]["workspaceId"]
            .as_str()
            .expect("id");

        let deleted = harness
            .post(
                "workspace.delete",
                serde_json::json!({ "workspaceId": workspace_id }),
            )
            .await;
        assert_eq!(deleted["result"]["ok"], true);
        assert_eq!(deleted["result"]["value"]["deleted"], true);

        let missing = harness
            .post(
                "workspace.delete",
                serde_json::json!({ "workspaceId": "no-such-workspace" }),
            )
            .await;
        assert_eq!(missing["result"]["ok"], false);
        assert_eq!(missing["result"]["error"]["code"], "workspace-not-found");

        let listed = harness.post("workspace.list", serde_json::json!({})).await;
        assert_eq!(
            listed["result"]["value"]["items"]
                .as_array()
                .expect("items")
                .len(),
            0
        );
    });
}

#[test]
fn create_rejects_a_missing_directory_with_workspace_invalid_path() {
    run(async {
        let harness = Harness::new();
        let missing = harness
            .root
            .join("does-not-exist")
            .to_string_lossy()
            .into_owned();
        let response = harness
            .post("workspace.create", serde_json::json!({ "path": missing }))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(
            response["result"]["error"]["code"],
            "workspace-invalid-path"
        );
    });
}

#[test]
fn archive_session_reports_unknown_sessions_as_session_not_found() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post(
                "workspace.archiveSession",
                serde_json::json!({ "sessionId": "ghost" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "session-not-found");

        // The known persisted session archives and appears in the list.
        let archived = harness
            .post(
                "workspace.archiveSession",
                serde_json::json!({ "sessionId": "s1" }),
            )
            .await;
        assert_eq!(archived["result"]["ok"], true, "{archived}");
        let ids = archived["result"]["value"]["archivedSessionIds"]
            .as_array()
            .expect("ids");
        assert!(ids.iter().any(|id| id == "s1"));

        let unarchived = harness
            .post(
                "workspace.unarchiveSession",
                serde_json::json!({ "sessionId": "s1" }),
            )
            .await;
        assert_eq!(unarchived["result"]["ok"], true);
        assert_eq!(
            unarchived["result"]["value"]["archivedSessionIds"]
                .as_array()
                .expect("ids")
                .len(),
            0
        );

        let archived_again = harness
            .post(
                "workspace.archiveSession",
                serde_json::json!({ "sessionId": "s1" }),
            )
            .await;
        assert_eq!(archived_again["result"]["ok"], true);
        let deleted = harness
            .post(
                "workspace.deleteArchivedSession",
                serde_json::json!({ "sessionId": "s1" }),
            )
            .await;
        assert_eq!(deleted["result"]["ok"], true, "{deleted}");
        assert_eq!(deleted["result"]["value"]["deleted"], true);
        assert!(
            deleted["result"]["value"]["archivedSessionIds"]
                .as_array()
                .expect("ids")
                .is_empty()
        );
    });
}

#[test]
fn a_missing_registry_is_internal() {
    run(async {
        let ctx = Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "workspace.list",
            "payload": {},
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/workspace.list".to_string(),
                query: vec![],
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: Some(body.into_bytes()),
            })
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("byte body");
        };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(parsed["result"]["ok"], false);
        assert_eq!(parsed["result"]["error"]["code"], "internal");
    });
}
