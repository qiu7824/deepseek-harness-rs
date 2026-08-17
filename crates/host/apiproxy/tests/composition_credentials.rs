//! Composition-layer `credentials.*` over the real fetch carrier with the
//! local provider: value-free describe, set/unset roundtrip, and the
//! rejection vocabulary.

use std::sync::Arc;

use dsh_credentials_local::{Config as CredentialConfig, LocalCredentialProvider};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn doc_path() -> String {
    std::env::temp_dir()
        .join(format!(
            "dsh-apiproxy-creds-{}-{}.yaml",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
        .to_string_lossy()
        .into_owned()
}

async fn post(
    handler: &dsh_host_apiproxy::FetchHandler,
    method: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let body = serde_json::to_string(&serde_json::json!({
        "type": "client-request",
        "rpcId": "r1",
        "method": method,
        "payload": payload,
    }))
    .expect("envelope");
    let response = handler
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

fn harness() -> (cordis::Context, dsh_host_apiproxy::FetchHandler, String) {
    let ctx = cordis::Context::root();
    let path = doc_path();
    LocalCredentialProvider::install(
        &ctx,
        CredentialConfig {
            path: Some(path.clone()),
            watch: Some(false),
            ..Default::default()
        },
    )
    .expect("provider");
    let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
    let handler = to_fetch_handler(service);
    (ctx, handler, path)
}

#[test]
fn describe_reports_value_free_state_and_set_unset_roundtrip() {
    run(async {
        let (_ctx, handler, path) = harness();
        let describe = post(
            &handler,
            "credentials.describe",
            serde_json::json!({ "refs": ["API_KEY", "UNSET_ONE"] }),
        )
        .await;
        assert_eq!(describe["result"]["ok"], true);
        let credentials = &describe["result"]["value"]["credentials"];
        assert_eq!(credentials["API_KEY"]["configured"], false);
        assert_eq!(credentials["API_KEY"]["writable"], true);
        assert_eq!(credentials["UNSET_ONE"]["configured"], false);

        // A set makes the reference configured (value itself never rides).
        let set = post(
            &handler,
            "credentials.set",
            serde_json::json!({ "ref": "API_KEY", "value": "sk-secret" }),
        )
        .await;
        assert_eq!(set["result"]["ok"], true);
        let describe = post(
            &handler,
            "credentials.describe",
            serde_json::json!({ "refs": ["API_KEY"] }),
        )
        .await;
        assert_eq!(
            describe["result"]["value"]["credentials"]["API_KEY"]["configured"],
            true
        );
        assert_eq!(
            describe["result"]["value"]["credentials"]["API_KEY"]["source"],
            "file"
        );

        // Unset is idempotent and clears the reference.
        let unset = post(
            &handler,
            "credentials.unset",
            serde_json::json!({ "ref": "API_KEY" }),
        )
        .await;
        assert_eq!(unset["result"]["ok"], true);
        let unset = post(
            &handler,
            "credentials.unset",
            serde_json::json!({ "ref": "API_KEY" }),
        )
        .await;
        assert_eq!(unset["result"]["ok"], true);

        let _ = std::fs::remove_file(&path);
    });
}

#[test]
fn an_invalid_reference_name_is_bad_request() {
    run(async {
        let (_ctx, handler, _path) = harness();
        let describe = post(
            &handler,
            "credentials.describe",
            serde_json::json!({ "refs": ["not-valid!"] }),
        )
        .await;
        assert_eq!(describe["result"]["ok"], false);
        assert_eq!(describe["result"]["error"]["code"], "bad-request");

        let set = post(
            &handler,
            "credentials.set",
            serde_json::json!({ "ref": "1bad", "value": "x" }),
        )
        .await;
        assert_eq!(set["result"]["ok"], false);
        assert_eq!(set["result"]["error"]["code"], "bad-request");
    });
}

#[test]
fn a_missing_credentials_service_is_internal() {
    run(async {
        let ctx = cordis::Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let describe = post(
            &handler,
            "credentials.describe",
            serde_json::json!({ "refs": ["API_KEY"] }),
        )
        .await;
        assert_eq!(describe["result"]["ok"], false);
        assert_eq!(describe["result"]["error"]["code"], "internal");
    });
}
