//! Composition-layer `session.history` over the real fetch carrier:
//! message-aligned pagination, beforeSeq windows, and the not-found
//! vocabulary.

use std::sync::Arc;

use cordis::Context;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, SessionEvent, SessionStore, SurfaceOp, session_id,
};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

fn user_message(seq: u64, time: i64, text: &str) -> SessionEvent {
    SessionEvent {
        type_: "user/message".to_string(),
        seq,
        time,
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

fn chunk(seq: u64, time: i64) -> SessionEvent {
    SessionEvent {
        type_: "message/chunk".to_string(),
        seq,
        time,
        data: serde_json::json!({}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn assistant_message(seq: u64, time: i64, sources: &[u64]) -> SessionEvent {
    SessionEvent {
        type_: "assistant/message".to_string(),
        seq,
        time,
        data: serde_json::json!({
            "message": {
                "id": format!("a{seq}"),
                "role": "assistant",
                "source": { "kind": "model", "provider": "stub", "model": "stub" },
                "content": [{ "type": "text", "text": "ok" }],
            }
        }),
        ignorable: None,
        surface_op: Some(SurfaceOp::Append),
        source_event_seqs: Some(sources.to_vec()),
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
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);
        Self {
            _ctx: ctx,
            handler,
            sessions,
        }
    }

    /// Seed a three-message session (user + assistant pairs, assistant
    /// folding its source).
    async fn seed(&self) {
        let _ = self
            .sessions
            .create(
                &self._ctx,
                Some(session_id("hist-1")),
                Some(CreateSessionOptions {
                    seed: Some(vec![
                        user_message(0, 100, "hello"),
                        assistant_message(1, 102, &[0]),
                        user_message(2, 200, "again"),
                        assistant_message(3, 202, &[2]),
                        user_message(4, 300, "third"),
                    ]),
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("seeded session");
    }

    async fn post(&self, payload: serde_json::Value) -> serde_json::Value {
        let body = serde_json::to_string(&serde_json::json!({
            "type": "client-request",
            "rpcId": "r1",
            "method": "session.history",
            "payload": payload,
        }))
        .expect("envelope");
        let response = self
            .handler
            .handle(CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.history".to_string(),
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
fn the_tail_page_cuts_at_message_boundaries() {
    run(async {
        let harness = Harness::new();
        harness.seed().await;
        let response = harness
            .post(serde_json::json!({ "sessionId": "hist-1", "maxMessages": 2 }))
            .await;
        assert_eq!(response["result"]["ok"], true, "{response}");
        let events = response["result"]["value"]["events"]
            .as_array()
            .expect("events");
        // Two complete messages back from the tail: the "again" pair plus
        // the trailing "third" user message (the seed-end marker rides the
        // same window); the "hello" pair is cut away.
        let texts: Vec<String> = events
            .iter()
            .filter(|entry| entry["event"]["type"] == "user/message")
            .map(|entry| {
                entry["event"]["data"]["content"][0]["text"]
                    .as_str()
                    .expect("text")
                    .to_string()
            })
            .collect();
        assert_eq!(texts, vec!["again".to_string(), "third".to_string()]);
        assert_eq!(response["result"]["value"]["hasMore"], true);
    });
}

#[test]
fn before_seq_windows_and_a_short_tail_are_not_more() {
    run(async {
        let harness = Harness::new();
        harness.seed().await;
        // beforeSeq = the second message's user seq (seed seqs start at
        // 0): only the first message pair remains.
        let response = harness
            .post(serde_json::json!({ "sessionId": "hist-1", "beforeSeq": 2, "maxMessages": 5 }))
            .await;
        let texts: Vec<String> = response["result"]["value"]["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter(|entry| entry["event"]["type"] == "user/message")
            .map(|entry| {
                entry["event"]["data"]["content"][0]["text"]
                    .as_str()
                    .expect("text")
                    .to_string()
            })
            .collect();
        assert_eq!(texts, vec!["hello".to_string()]);
        assert_eq!(response["result"]["value"]["hasMore"], false);

        // The whole log fits under the default bound: not more.
        let tail = harness
            .post(serde_json::json!({ "sessionId": "hist-1" }))
            .await;
        assert_eq!(tail["result"]["value"]["hasMore"], false);
        let tail_texts: Vec<String> = tail["result"]["value"]["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter(|entry| entry["event"]["type"] == "user/message")
            .map(|entry| {
                entry["event"]["data"]["content"][0]["text"]
                    .as_str()
                    .expect("text")
                    .to_string()
            })
            .collect();
        assert_eq!(
            tail_texts,
            vec![
                "hello".to_string(),
                "again".to_string(),
                "third".to_string(),
            ]
        );
    });
}

#[test]
fn an_unknown_session_is_session_not_found() {
    run(async {
        let harness = Harness::new();
        harness.seed().await;
        let response = harness
            .post(serde_json::json!({ "sessionId": "ghost" }))
            .await;
        assert_eq!(response["result"]["ok"], false);
        assert_eq!(response["result"]["error"]["code"], "session-not-found");
    });
}
