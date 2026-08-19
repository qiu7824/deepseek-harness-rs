//! Composition-layer `session.search` over the real fetch carrier with a
//! stub search backend: authorization filtering and the service-absence
//! vocabulary.

use std::sync::Arc;

use cordis::Context;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, SessionEvent, SessionHeader, SessionId, SessionStore,
    SurfaceOp, session_id,
};
use dsh_session_query::{
    Config as QueryConfig, SessionEventSearchHit, SessionEventSearchPage, SessionEventSurface,
    SessionQueryEngine, SessionQueryError, SessionQuerySearch, SessionSearchExecContext,
    SessionSearchHit, SessionSearchPage, SessionSearchRequest,
};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Search backend serving one fixed hit.
struct StubSearch;

#[async_trait::async_trait]
impl SessionQuerySearch for StubSearch {
    async fn search_sessions(
        &self,
        _engine: &SessionQueryEngine,
        _request: &SessionSearchRequest,
        _exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionSearchPage<SessionSearchHit>, SessionQueryError> {
        Ok(SessionSearchPage {
            items: vec![SessionSearchHit {
                record: dsh_session_query::SessionRecord {
                    header: SessionHeader {
                        version: 1,
                        id: session_id("hit-1"),
                        created_at: 0,
                        cwd: Some("D:\\hit".to_string()),
                        parent_session: None,
                        seed_length: None,
                        origin: None,
                        delegation_depth: None,
                        agent_preset: None,
                    },
                    live: true,
                    persisted: false,
                },
                best_match: SessionEventSearchHit {
                    session_id: session_id("hit-1"),
                    seq: 0,
                    type_: "user/message".to_string(),
                    time: 0,
                    surface: SessionEventSurface::Current,
                    snippet: "the matching snippet".to_string(),
                },
            }],
            next_cursor: None,
        })
    }

    async fn search_events(
        &self,
        _engine: &SessionQueryEngine,
        _request: &dsh_session_query::SessionEventSearchRequest,
        _exec: Option<&SessionSearchExecContext>,
    ) -> Result<SessionEventSearchPage, SessionQueryError> {
        Err(SessionQueryError::new(
            dsh_session_query::SessionQueryErrorCode::SessionQuerySearchDisabled,
            "unused",
        ))
    }
}

fn text_event(seq: u64, text: &str) -> SessionEvent {
    SessionEvent {
        type_: "user/message".to_string(),
        seq,
        time: seq as i64,
        data: serde_json::json!({
            "id": format!("u{seq}"),
            "role": "user",
            "source": { "kind": "user" },
            "content": [{ "type": "text", "text": text }],
        }),
        ignorable: None,
        surface_op: Some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}

struct Harness {
    _ctx: Context,
    handler: dsh_host_apiproxy::FetchHandler,
    sessions: Arc<SessionStore>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        SessionQueryEngine::install(&ctx, &QueryConfig::default(), Some(Arc::new(StubSearch)))
            .expect("query engine");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            sessions,
        }
    }

    async fn seed(&self, id: &str, events: Vec<SessionEvent>) {
        let _ = self
            .sessions
            .create(
                &self._ctx,
                Some(session_id(id)),
                Some(CreateSessionOptions {
                    seed: Some(events),
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");
    }

    async fn post(&self, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "session.search",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.search".to_string(),
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
fn search_emits_only_hits_naming_visible_sessions() {
    run(async {
        let harness = Harness::new();
        // The stub hit names hit-1, which is NOT attached: the visible set
        // is empty, so nothing is emitted.
        let empty = harness.post(serde_json::json!({ "query": "x" })).await;
        assert_eq!(empty["result"]["ok"], true, "{empty}");
        assert_eq!(
            empty["result"]["value"]["items"]
                .as_array()
                .expect("items")
                .len(),
            0
        );
        assert_eq!(empty["result"]["value"]["hasMore"], false);

        // Attach hit-1: the stub hit now passes the authorization boundary.
        harness
            .seed("hit-1", vec![text_event(0, "matching text")])
            .await;
        let found = harness.post(serde_json::json!({ "query": "x" })).await;
        assert_eq!(found["result"]["ok"], true, "{found}");
        let items = found["result"]["value"]["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["sessionId"], "hit-1");
        assert_eq!(items[0]["snippet"], "the matching snippet");
    });
}

#[test]
fn a_missing_query_engine_is_internal() {
    run(async {
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "session.search",
            "payload": { "query": "x" },
        }))
        .expect("envelope");
        let response = handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.search".to_string(),
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

#[allow(unused)]
fn _vocab() {
    let _: Option<dsh_session_query::SessionSearchCursor> = None;
    let _: Option<SessionId> = None;
}
