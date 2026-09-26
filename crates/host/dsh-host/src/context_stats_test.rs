//! Exercise the composed Host and its actual history RPC, so a projection
//! that exists as a library but is omitted from boot cannot pass unnoticed.

use super::*;

#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composed_host_releases_real_agents_after_scope_teardown() {
    use dsh_agent::{AgentFactory, AgentOptions, CreateAgentOptions};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Adapter(AtomicUsize);
    impl dsh_llm::LlmAdapter for Adapter {
        fn stream(&self, _: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(futures::stream::iter(vec![dsh_llm::StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                replay_state: None,
            }]))
        }
    }
    let directory = std::env::temp_dir().join(format!("host-retention-{}", uuid::Uuid::new_v4()));
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &directory, None).unwrap();
    let adapter = Arc::new(Adapter(AtomicUsize::new(0)));
    host.llm
        .register_adapter(&ctx, vec!["memory-fixture".into()], adapter.clone())
        .unwrap();
    let handler = to_fetch_handler(host.api_proxy.clone());
    for generation in 0..8 {
        let handle = host
            .agent_loop
            .create_agent(
                &ctx,
                CreateAgentOptions {
                    agent_options: Some(AgentOptions {
                        provider: Some("memory-fixture".into()),
                        model: Some("fixture".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let weak = Arc::downgrade(&handle.agent);
        handle.agent.followup(dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "scope lifecycle".into(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while adapter.0.load(Ordering::SeqCst) <= generation {
                tokio::task::yield_now().await;
            }
            handle.agent.when_idle().await;
        })
        .await
        .unwrap();
        host.sessions.flush(handle.agent.session()).await.unwrap();
        handle.dispose.await;
        drop(handle.agent);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "composed Host retained generation {generation}: {} references",
                weak.strong_count()
            )
        });
    }
    for _ in 0..8 {
        let before = adapter.0.load(Ordering::SeqCst);
        let created=handler.handle(CarrierRequest{method:http::Method::POST,path:"/api/session.create".into(),query:vec![],headers:vec![("content-type".into(),"application/json".into())],body:Some(serde_json::json!({"type":"client-request","rpcId":"memory-create","method":"session.create","payload":{"cwd":directory.to_string_lossy(),"provider":"memory-fixture","model":"fixture"}}).to_string().into_bytes())}).await;
        let value: serde_json::Value = match created.into_body() {
            CarrierBody::Bytes(bytes) => serde_json::from_slice(&bytes).unwrap(),
            _ => panic!("unary"),
        };
        assert_eq!(value["result"]["ok"], true, "{value}");
        let id = dsh_session::session_id(value["result"]["value"]["sessionId"].as_str().unwrap());
        let agent = host.agents.get(&id).unwrap();
        let weak = Arc::downgrade(&agent);
        let _=handler.handle(CarrierRequest{method:http::Method::POST,path:"/api/session.selectModel".into(),query:vec![],headers:vec![("content-type".into(),"application/json".into())],body:Some(serde_json::json!({"type":"client-request","rpcId":"memory-model","method":"session.selectModel","payload":{"sessionId":id,"provider":"memory-fixture","model":"fixture"}}).to_string().into_bytes())}).await;
        let response=handler.handle(CarrierRequest{method:http::Method::POST,path:"/api/session.prompt".into(),query:vec![],headers:vec![("content-type".into(),"application/json".into())],body:Some(serde_json::json!({"type":"client-request","rpcId":"memory-prompt","method":"session.prompt","payload":{"sessionId":id,"mode":"queue","content":[{"type":"text","text":"memory regression"}]}}).to_string().into_bytes())}).await;
        let value: serde_json::Value = match response.into_body() {
            CarrierBody::Bytes(bytes) => serde_json::from_slice(&bytes).unwrap(),
            _ => panic!("unary"),
        };
        assert_eq!(value["result"]["ok"], true, "{value}");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while adapter.0.load(Ordering::SeqCst) == before {
                tokio::task::yield_now().await;
            }
            agent.when_idle().await;
        })
        .await
        .unwrap();
        let session_lifetime = agent.session().debug_lifetime();
        drop(agent);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("API retained agent: {}", weak.strong_count()));
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while session_lifetime.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("API retained Session: {}", session_lifetime.strong_count()));
    }
    host.shutdown().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

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
    let bytes = match response.into_body() {
        CarrierBody::Bytes(bytes) => bytes,
        CarrierBody::Stream(mut stream) => {
            use futures::StreamExt;
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                bytes.extend(chunk.expect("JSON history chunk"));
            }
            bytes
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON history RPC");
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
