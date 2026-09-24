use super::idle_retirement_tests::control_admission_fixture;
use super::*;

async fn activity(service: &Arc<ApiProxyService>, id: &str) -> bool {
    let response = crate::fetch::handler::to_fetch_handler(service.clone())
        .handle(crate::fetch::handler::CarrierRequest {
            method: http::Method::POST,
            path: "/api/commands.activity".into(),
            query: vec![],
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(
                serde_json::to_vec(&serde_json::json!({
                    "type":"client-request","rpcId":"command-activity-test",
                    "method":"commands.activity","payload":{"sessionId":id}
                }))
                .unwrap(),
            ),
        })
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let Body::Bytes(bytes) = response.into_body() else {
        panic!("unary status must return bytes")
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");
    value["result"]["value"]["active"].as_bool().unwrap()
}

#[tokio::test]
async fn command_activity_reports_only_current_owner_work_without_resolving_cold_sessions() {
    let (service, agent, detach) = control_admission_fixture("command-activity-owner").await;
    let commands = dsh_commands::CommandRuntime::install(&service.ctx);
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let _registration = commands
        .register(
            &service.ctx,
            dsh_commands::CommandDefinition {
                name: "wait".into(),
                description: "Controlled wait".into(),
                input: None,
                record_input: Some(false),
                handler: Arc::new(move |_| {
                    let notify = notify.clone();
                    Box::pin(async move {
                        notify.notify_one();
                        std::future::pending().await
                    })
                }),
            },
        )
        .unwrap();
    assert!(!activity(&service, agent.id().as_str()).await);
    let owner = agent.clone();
    let worker =
        tokio::spawn(async move { commands.execute(&owner, "/wait", Arc::new(|| false)).await });
    started.notified().await;
    assert!(activity(&service, agent.id().as_str()).await);
    assert!(!activity(&service, "cold-no-command-owner").await);
    assert!(
        service
            .agents()
            .unwrap()
            .get(&dsh_session::session_id("cold-no-command-owner"))
            .is_none()
    );
    worker.abort();
    let _ = worker.await;
    assert!(!activity(&service, agent.id().as_str()).await);
    detach().await;
}
