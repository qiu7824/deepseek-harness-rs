use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_has_no_task_contract_tools_endpoint_database_or_context() {
    let root = std::env::temp_dir().join(format!("retired-task-contract-{}", uuid::Uuid::new_v4()));
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &root, Some("web")).unwrap();
    assert!(host.tools.get("task_execution", None).is_none());
    assert!(host.tools.get("skill_candidate", None).is_none());
    for name in ["office_render", "workspace_scratch"] {
        assert!(host.tools.get(name, None).is_some(), "missing {name}");
    }
    let prompt = host
        .system_prompt
        .assemble(&ctx, &Default::default())
        .await
        .unwrap();
    assert!(
        !prompt
            .sections
            .iter()
            .any(|section| section.name == "task:acceptance")
    );
    assert!(
        !prompt
            .contexts
            .iter()
            .any(|section| section.name == "task:durable-state")
    );
    assert!(!root.join("task-execution-v1.sqlite").exists());
    let base = format!("http://127.0.0.1:{}", host.web_server.port());
    let response = reqwest::Client::new()
        .post(format!("{base}/__dsh-task-execution"))
        .header("Origin", &base)
        .json(&serde_json::json!({"action":"list","sessionId":"retired"}))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 404 | 405));
    host.shutdown().await.unwrap();
    drop(host);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
