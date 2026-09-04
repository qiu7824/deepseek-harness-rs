//! Exercise the composed Host and its actual history RPC, so a projection
//! that exists as a library but is omitted from boot cannot pass unnoticed.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composed_host_history_exposes_context_insights_and_budget_provenance() {
    let directory = std::env::temp_dir().join(format!("dsh-context-host-{}", uuid::Uuid::new_v4()));
    let context = Context::root();
    let host = compose_persistent_host_at(&context, &directory, None).expect("compose host");
    let session = host
        .sessions
        .create(
            &host.ctx,
            None,
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create session");
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "Context statistics check".into(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append user message");
    session
        .append(
            "request/context",
            serde_json::json!({"contextWindow":131072,"contextWindowEstimated":true}),
            None,
        )
        .expect("append runtime budget");
    host.sessions
        .flush(&session)
        .await
        .expect("flush durable history");

    let handler = to_fetch_handler(host.api_proxy.clone());
    let response = handler.handle(CarrierRequest {
        method: http::Method::POST,
        path: "/api/session.history".into(),
        query: Vec::new(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: Some(serde_json::to_vec(&serde_json::json!({
            "type":"client-request", "rpcId":"context-stats-regression", "method":"session.history",
            "payload":{"sessionId":session.id()}
        })).unwrap()),
    }).await;
    let status = response.status();
    let value: serde_json::Value = match response.into_body() {
        CarrierBody::Bytes(bytes) => serde_json::from_slice(&bytes).expect("JSON history RPC"),
        CarrierBody::Stream(_) => panic!("history must be a unary response"),
    };
    host.shutdown().await.expect("drain host");
    std::fs::remove_dir_all(directory).expect("remove temporary home");

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(value["result"]["ok"], true, "{value}");
    let projections = &value["result"]["value"]["projections"]["values"];
    assert_eq!(projections["contextInsights"]["userMessages"], 1);
    assert_eq!(projections["contextInsights"]["assistantMessages"], 0);
    assert!(
        projections["contextInsights"]["createdAt"]
            .as_u64()
            .is_some()
    );
    assert!(
        projections["contextInsights"]["roleTokens"]["user"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(projections["sessionStats"].is_object());
    assert_eq!(projections["contextPressure"]["contextWindow"], 131072);
    assert_eq!(
        projections["contextPressure"]["contextWindowEstimated"],
        true
    );
}
