//! Composition-layer host domain over the real fetch carrier: the
//! describe snapshot and the browse-capability directory methods, driven
//! through `POST /api/host.*` envelopes.

use std::sync::Arc;

use dsh_agent::AgentRegistry;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, ModelSelection, to_fetch_handler,
};
use dsh_host_directory_picker::{DirectoryPicker, DirectoryPickerCapability};
use dsh_host_directory_picker_browse::{BrowseDirectoryPicker, Config};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

struct Harness {
    _ctx: cordis::Context,
    handler: dsh_host_apiproxy::FetchHandler,
    root: std::path::PathBuf,
    picker: Arc<BrowseDirectoryPicker>,
}

impl Harness {
    fn new() -> Self {
        let ctx = cordis::Context::root();
        AgentRegistry::install(&ctx);
        let picker = BrowseDirectoryPicker::install(&ctx, Config::default());
        assert!(matches!(
            picker.capability(),
            DirectoryPickerCapability::Browse(_)
        ));
        let service = ApiProxyService::install(
            &ctx,
            ApiProxyDefaults {
                default_model_selection: Arc::new(|| ModelSelection {
                    provider: "deepseek-official".to_string(),
                    model: "deepseek-chat".to_string(),
                    reasoning_effort: None,
                }),
                cwd: "D:\\proj".to_string(),
                ..Default::default()
            },
        );
        let handler = to_fetch_handler(service);
        let root = std::env::temp_dir().join(format!(
            "dsh-apiproxy-host-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::create_dir(root.join("projects")).expect("child");
        Self {
            _ctx: ctx,
            handler,
            root,
            picker,
        }
    }

    async fn post(&self, path: &str, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": path.strip_prefix("/api/").expect("api path"),
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: path.to_string(),
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
fn describe_reports_the_host_snapshot() {
    run(async {
        let harness = Harness::new();
        let response = harness.post("/api/host.describe", serde_json::json!({})).await;
        assert_eq!(response["type"], "server-response");
        assert_eq!(response["rpcId"], "r1");
        assert_eq!(response["result"]["ok"], true);
        let value = &response["result"]["value"];
        assert_eq!(value["version"], "0.0.1");
        assert_eq!(value["cwd"], "D:\\proj");
        assert_eq!(value["provider"], "deepseek-official");
        assert_eq!(value["model"], "deepseek-chat");
        assert_eq!(value["attachedSessions"], 0);
        // No injected opener and no probe: fall back to false.
        assert_eq!(value["canOpenPath"], false);
    });
}

#[test]
fn list_directory_serves_the_browse_backend() {
    run(async {
        let harness = Harness::new();
        let path = harness.root.to_string_lossy().into_owned();
        let response = harness
            .post("/api/host.listDirectory", serde_json::json!({ "path": path }))
            .await;
        assert_eq!(response["result"]["ok"], true);
        let value = &response["result"]["value"];
        assert_eq!(value["path"], path);
        let names: Vec<&str> = value["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["projects"]);
        assert_eq!(value["truncated"], false);
    });
}

#[test]
fn create_directory_writes_and_surfaces_the_child() {
    run(async {
        let harness = Harness::new();
        let path = harness.root.to_string_lossy().into_owned();
        let response = harness
            .post(
                "/api/host.createDirectory",
                serde_json::json!({ "path": path, "name": "fresh" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], true);
        assert_eq!(
            response["result"]["value"]["path"],
            format!("{}\\fresh", path)
        );

        let listing = harness
            .post("/api/host.listDirectory", serde_json::json!({ "path": path }))
            .await;
        let names: Vec<&str> = listing["result"]["value"]["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["fresh", "projects"]);
    });
}

#[test]
fn pick_directory_under_a_browse_backend_is_picker_unavailable() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post("/api/host.pickDirectory", serde_json::json!({}))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(
            response["result"]["error"]["code"],
            "directory-picker-unavailable"
        );
        assert_eq!(
            response["result"]["error"]["details"]["capability"],
            "browse"
        );
    });
}

#[test]
fn create_directory_reports_directory_exists() {
    run(async {
        let harness = Harness::new();
        let path = harness.root.to_string_lossy().into_owned();
        let response = harness
            .post(
                "/api/host.createDirectory",
                serde_json::json!({ "path": path, "name": "projects" }),
            )
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "directory-exists");
    });
}

#[test]
fn unwired_domains_answer_a_named_internal_error() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post("/api/session.list", serde_json::json!({}))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "internal");
        assert!(
            response["result"]["error"]["message"]
                .as_str()
                .expect("message")
                .contains("session.list")
        );
    });
}

#[test]
fn open_path_without_an_injected_opener_is_internal() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post("/api/host.openPath", serde_json::json!({ "path": "D:\\x" }))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "internal");
    });
}

#[test]
fn list_directory_rejects_a_non_fully_qualified_path_with_directory_unreadable() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post("/api/host.listDirectory", serde_json::json!({ "path": "relative" }))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(
            response["result"]["error"]["code"],
            "directory-unreadable"
        );
    });
}
