//! Composition-layer `settings.*` over the real fetch carrier with an
//! in-memory storage: describe redaction, update/replace/mutate writes, and
//! the conflict/rejection vocabulary.

use std::sync::Arc;

use cordis::Context;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_settings::{
    SettingsNamespace, SettingsProvider, SettingsRegisterOptions, SettingsStorage,
};
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemastery::{Data, Schema};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn ns(name: &str) -> SettingsNamespace {
    dsh_settings::settings_namespace(name).expect("namespace")
}

fn theme_schema() -> Schema {
    let mut properties = IndexMap::new();
    properties.insert(
        "theme".to_string(),
        Schema::union(vec![
            Schema::constant(Data::String("dark".to_string())),
            Schema::constant(Data::String("light".to_string())),
        ])
        .default(Data::String("dark".to_string())),
    );
    properties.insert(
        "font_size".to_string(),
        Schema::number().default(Data::Number(14.0)),
    );
    Schema::object(properties)
}

fn json_to_data(value: &serde_json::Value) -> Data {
    match value {
        serde_json::Value::Null => Data::Null,
        serde_json::Value::Bool(value) => Data::Bool(*value),
        serde_json::Value::Number(value) => Data::Number(value.as_f64().unwrap()),
        serde_json::Value::String(value) => Data::String(value.clone()),
        serde_json::Value::Array(array) => Data::Array(array.iter().map(json_to_data).collect()),
        serde_json::Value::Object(object) => {
            let mut entries = IndexMap::new();
            for (key, value) in object {
                entries.insert(key.clone(), json_to_data(value));
            }
            Data::Object(entries)
        }
    }
}

struct MemorySettings {
    doc: Mutex<IndexMap<String, Data>>,
}

#[async_trait::async_trait]
impl SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        true
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.doc.lock().clone())
    }

    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String> {
        self.doc.lock().insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let storage = Arc::new(MemorySettings {
            doc: Mutex::new(IndexMap::new()),
        });
        let provider = SettingsProvider::install(&ctx, storage);
        provider
            .register(
                &ctx,
                ns("ui-theme"),
                theme_schema(),
                SettingsRegisterOptions::default(),
            )
            .expect("register");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self { _ctx: ctx, handler }
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

#[test]
fn describe_reports_registered_namespaces_with_revisions() {
    run(async {
        let harness = Harness::new();
        let response = harness.post("settings.describe", serde_json::json!({})).await;
        assert_eq!(response["result"]["ok"], true);
        let value = &response["result"]["value"];
        assert_eq!(value["writable"], true);
        assert_eq!(value["hasDocument"], false);
        let namespaces = value["namespaces"].as_array().expect("namespaces");
        assert_eq!(namespaces.len(), 1);
        let view = &namespaces[0];
        assert_eq!(view["ns"], "ui-theme");
        assert_eq!(view["value"]["theme"], "dark");
        assert_eq!(view["value"]["font_size"], 14.0);
        assert_eq!(view["applies"], "live");
        assert_eq!(view["revision"], 0);
    });
}

#[test]
fn update_merges_a_patch_and_answers_the_new_view() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post(
                "settings.update",
                serde_json::json!({ "ns": "ui-theme", "patch": { "theme": "light" } }),
            )
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        let view = &response["result"]["value"];
        assert_eq!(view["ns"], "ui-theme");
        assert_eq!(view["value"]["theme"], "light");
        assert_eq!(view["value"]["font_size"], 14.0, "merge preserves the rest");
        assert_eq!(view["revision"], 1);
    });
}

#[test]
fn replace_resets_the_whole_section() {
    run(async {
        let harness = Harness::new();
        // First write: a whole-section replacement.
        let response = harness
            .post(
                "settings.replace",
                serde_json::json!({ "ns": "ui-theme", "section": { "theme": "light" } }),
            )
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        let view = &response["result"]["value"];
        assert_eq!(view["value"]["theme"], "light");
        assert_eq!(view["revision"], 1);

        // Resetting to the empty section returns to schema defaults (the
        // raw section changes, so the revision advances).
        let reset = harness
            .post(
                "settings.replace",
                serde_json::json!({ "ns": "ui-theme", "section": {} }),
            )
            .await;
        assert_eq!(reset["result"]["ok"], true, "{reset}");
        assert_eq!(reset["result"]["value"]["value"]["theme"], "dark");
        assert_eq!(reset["result"]["value"]["revision"], 2);
    });
}

#[test]
fn mutate_applies_path_ops_and_reports_rejection() {
    run(async {
        let harness = Harness::new();
        let response = harness
            .post(
                "settings.mutate",
                serde_json::json!({
                    "ns": "ui-theme",
                    "ops": [{ "op": "set", "path": ["font_size"], "value": 20 }],
                }),
            )
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        assert_eq!(response["result"]["value"]["value"]["font_size"], 20.0);

        // An unknown namespace is a settings-rejected, not a conflict.
        let rejected = harness
            .post(
                "settings.update",
                serde_json::json!({ "ns": "ui-absent", "patch": {} }),
            )
            .await;
        assert_eq!(rejected["result"]["ok"], false);
        assert_eq!(rejected["result"]["error"]["code"], "settings-rejected");
        assert_eq!(rejected["result"]["error"]["details"]["ns"], "ui-absent");
    });
}

#[test]
fn a_stale_expected_revision_is_settings_conflict() {
    run(async {
        let harness = Harness::new();
        let first = harness
            .post(
                "settings.update",
                serde_json::json!({ "ns": "ui-theme", "patch": { "theme": "light" } }),
            )
            .await;
        assert_eq!(first["result"]["ok"], true);
        // The next write claims the pre-write revision 0: conflict.
        let conflicted = harness
            .post(
                "settings.update",
                serde_json::json!({
                    "ns": "ui-theme",
                    "patch": { "theme": "dark" },
                    "expectedRevision": 0,
                }),
            )
            .await;
        assert_eq!(conflicted["result"]["ok"], false, "{conflicted}");
        assert_eq!(conflicted["result"]["error"]["code"], "settings-conflict");
        assert_eq!(conflicted["result"]["error"]["details"]["ns"], "ui-theme");
        assert_eq!(conflicted["result"]["error"]["details"]["expected"], 0);
        assert_eq!(conflicted["result"]["error"]["details"]["actual"], 1);
    });
}

#[test]
fn a_missing_settings_service_is_internal() {
    run(async {
        let ctx = Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "settings.describe",
            "payload": {},
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/settings.describe".to_string(),
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
