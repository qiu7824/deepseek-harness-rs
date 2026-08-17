//! Composition-layer `skill.list` over the real fetch carrier: session
//! resolution, service absence errors, and the user-invocable catalog
//! projection.

use std::sync::Arc;

use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, SessionStore, session_id,
};
use dsh_skill::{
    Config as SkillConfig, SkillInvocationPolicy, SkillRegistration, SkillRegistry,
};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn cwd() -> String {
    std::env::temp_dir()
        .join(format!(
            "dsh-apiproxy-skill-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
        .to_string_lossy()
        .into_owned()
}

async fn post(handler: &dsh_host_apiproxy::FetchHandler, payload: serde_json::Value) -> serde_json::Value {
    let body = serde_json::to_string(&serde_json::json!({
        "type": "client-request",
        "rpcId": "r1",
        "method": "skill.list",
        "payload": payload,
    }))
    .expect("envelope");
    let response = handler
        .handle(CarrierRequest {
            method: http::Method::POST,
            path: "/api/skill.list".to_string(),
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

#[test]
fn lists_the_user_invocable_catalog_for_the_sessions_cwd() {
    run(async {
        let ctx = cordis::Context::root();
        let sessions = SessionStore::install(&ctx);
        let session = sessions
            .create(
                &ctx,
                Some(session_id("s1")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some(cwd()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");
        let _ = session;
        let skills = SkillRegistry::install(&ctx, SkillConfig::default()).expect("registry");
        skills
            .register(
                &ctx,
                SkillRegistration {
                    name: "deploy".to_string(),
                    description: "Ship it".to_string(),
                    when_to_use: Some("when ready".to_string()),
                    source: "test".to_string(),
                    resource_base: None,
                    content: "body".to_string(),
                    path: None,
                    metadata: None,
                    invocation: Some(SkillInvocationPolicy::BOTH),
                    provider: None,
                },
            );
        skills
            .register(
                &ctx,
                SkillRegistration {
                    name: "secret-op".to_string(),
                    description: "user-only".to_string(),
                    when_to_use: None,
                    source: "test".to_string(),
                    resource_base: None,
                    content: "body".to_string(),
                    path: None,
                    metadata: None,
                    // Model-only: filtered out of the user catalog.
                    invocation: Some(SkillInvocationPolicy {
                        model_invocable: true,
                        user_invocable: false,
                    }),
                    provider: None,
                },
            );
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);

        let response = post(&handler, serde_json::json!({ "sessionId": "s1" })).await;
        assert_eq!(response["result"]["ok"], true);
        let skills_json = response["result"]["value"]["skills"]
            .as_array()
            .expect("skills");
        assert_eq!(skills_json.len(), 1, "model-only skills are filtered");
        let row = &skills_json[0];
        assert_eq!(row["name"], "deploy");
        assert_eq!(row["description"], "Ship it");
        assert_eq!(row["whenToUse"], "when ready");
        assert_eq!(row["modelInvocable"], true);
    });
}

#[test]
fn an_unattached_session_is_session_not_found() {
    run(async {
        let ctx = cordis::Context::root();
        SessionStore::install(&ctx);
        SkillRegistry::install(&ctx, SkillConfig::default()).expect("registry");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);

        let response = post(&handler, serde_json::json!({ "sessionId": "missing" })).await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "session-not-found");
        assert_eq!(
            response["result"]["error"]["details"]["sessionId"],
            "missing"
        );
    });
}

#[test]
fn a_missing_skill_registry_is_internal() {
    run(async {
        let ctx = cordis::Context::root();
        let sessions = SessionStore::install(&ctx);
        let _ = sessions
            .create(
                &ctx,
                Some(session_id("s1")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some(cwd()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);

        let response = post(&handler, serde_json::json!({ "sessionId": "s1" })).await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "internal");
    });
}

#[test]
fn a_missing_sessions_service_is_internal() {
    run(async {
        let ctx = cordis::Context::root();
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let response = post(&handler, serde_json::json!({ "sessionId": "s1" })).await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "internal");
    });
}
