//! Composition-layer mux stream over the real fetch carrier: the
//! subscribed baseline and live session-event frames.

use std::sync::Arc;

use cordis::Context;
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_session::{CreateSessionMeta, CreateSessionOptions, SessionStore, SurfaceOp, session_id};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

async fn open_mux(
    handler: &dsh_host_apiproxy::FetchHandler,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + Send>> {
    let response = handler
        .handle(CarrierRequest {
            method: http::Method::GET,
            path: "/api/events.mux".to_string(),
            query: vec![],
            headers: vec![],
            body: None,
        })
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let Body::Stream(stream) = response.into_body() else {
        panic!("SSE answers are stream bodies");
    };
    stream
}

#[test]
fn the_mux_stream_opens_with_the_subscribed_baseline_and_forwards_events() {
    run(async {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let attached = sessions
            .create(
                &ctx,
                Some(session_id("mux-1")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);

        let stream = open_mux(&handler).await;
        let mut frames = Box::pin(futures::StreamExt::take(stream, 3));
        use futures::StreamExt;
        let first = frames
            .next()
            .await
            .expect("open comment")
            .expect("open stream success");
        let text = String::from_utf8(first).expect("utf8");
        assert!(text.starts_with(": connected\n\n"), "{text}");

        // The subscribed baseline frame.
        let second = frames
            .next()
            .await
            .expect("baseline")
            .expect("baseline success");
        let text = String::from_utf8(second).expect("utf8");
        let payload: serde_json::Value = serde_json::from_str(
            text.lines()
                .find(|line| line.starts_with("data: "))
                .expect("data line")
                .trim_start_matches("data: "),
        )
        .expect("frame json");
        assert_eq!(payload["type"], "server-request");
        assert_eq!(payload["method"], "session/subscribed");
        assert_eq!(payload["payload"]["sessionId"], "mux-1");
        assert_eq!(
            payload["payload"]["lastSeq"], -1,
            "an empty log's last durable sequence is -1; seq() is the next sequence"
        );

        // Let the spawned listener registration settle before appending
        // (the cordis `on` await runs in a spawned task).
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        // Append a user message: the live frame rides the stream.
        let _ = attached
            .append(
                "user/message",
                serde_json::json!({
                    "id": "u1",
                    "role": "user",
                    "source": { "kind": "user" },
                    "content": [{ "type": "text", "text": "hi" }],
                }),
                Some(dsh_session::SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .expect("append");
        let third = frames
            .next()
            .await
            .expect("live frame")
            .expect("live frame success");
        let text = String::from_utf8(third).expect("utf8");
        let payload: serde_json::Value = serde_json::from_str(
            text.lines()
                .find(|line| line.starts_with("data: "))
                .expect("data line")
                .trim_start_matches("data: "),
        )
        .expect("frame json");
        assert_eq!(payload["method"], "session/event");
        assert_eq!(payload["payload"]["type"], "session/event");
        assert_eq!(payload["payload"]["sessionId"], "mux-1");
        assert_eq!(payload["payload"]["event"]["type"], "user/message");
    });
}
