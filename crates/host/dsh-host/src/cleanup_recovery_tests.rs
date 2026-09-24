use super::*;
use dsh_agent::{AgentFactory, CreateAgentOptions};
use serde_json::{Value, json};

async fn post(host: &crate::HostSpine, operation: &str, args: Value) -> (http::StatusCode, Value) {
    let base = format!("http://{}", host.readiness().bound_addr);
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}/__dsh-artifacts/{operation}"))
        .header("Origin", &base)
        .json(&args)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let value = response.json().await.unwrap();
    (status, value)
}
async fn ok(host: &crate::HostSpine, operation: &str, args: Value) -> Value {
    let (status, value) = post(host, operation, args).await;
    assert!(status.is_success(), "{operation}: {status} {value}");
    value
}
async fn scratch(host: &crate::HostSpine, agent: Arc<dyn dsh_agent::Agent>, args: Value) -> Value {
    let result = host
        .tools
        .execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: "workspace_scratch".into(),
            arguments: args,
            agent: Some(agent),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    assert!(!result.is_error, "{:?}", result.error);
    result.value.clone().unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_scope_evidence_recovery_locations_and_offline_roots_follow_real_host_api() {
    let root = std::env::temp_dir().join(format!("cleanup-host-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    let project_a = root.join("project-a");
    let project_b = root.join("project-b");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();
    let ctx = Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &home, None).unwrap();
    let registry = ctx
        .get_typed::<Arc<WorkspaceRegistry>>("workspaceRegistry", false)
        .unwrap()
        .as_ref()
        .clone();
    let workspace_a = registry
        .create(&project_a.to_string_lossy(), None)
        .await
        .unwrap();
    let workspace_b = registry
        .create(&project_b.to_string_lossy(), None)
        .await
        .unwrap();
    let create = |id: &str, path: &Path| CreateAgentOptions {
        session_id: Some(session_id(id)),
        meta: Some(dsh_session::CreateSessionMeta {
            cwd: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = host
        .agent_loop
        .create_agent(&ctx, create("cleanup-a", &project_a))
        .await
        .unwrap();
    let b = host
        .agent_loop
        .create_agent(&ctx, create("cleanup-b", &project_b))
        .await
        .unwrap();
    workspace_a.attach_session(a.agent.id()).await.unwrap();
    workspace_b.attach_session(b.agent.id()).await.unwrap();
    let first = scratch(
        &host,
        a.agent.clone(),
        json!({"action":"allocate","kind":"script","label":"task A"}),
    )
    .await;
    let second = scratch(
        &host,
        b.agent.clone(),
        json!({"action":"allocate","kind":"script","label":"task B"}),
    )
    .await;
    scratch(
        &host,
        a.agent.clone(),
        json!({"action":"write","id":first["id"],"path":"a.txt","content":"A"}),
    )
    .await;
    scratch(
        &host,
        b.agent.clone(),
        json!({"action":"write","id":second["id"],"path":"b.txt","content":"B"}),
    )
    .await;
    scratch(
        &host,
        a.agent.clone(),
        json!({"action":"release","id":first["id"]}),
    )
    .await;
    scratch(
        &host,
        b.agent.clone(),
        json!({"action":"release","id":second["id"]}),
    )
    .await;
    ok(&host, "list", json!({"sessionId":"cleanup-a"})).await;
    let nested = project_a.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("report.txt"), b"original report").unwrap();
    let listing = ok(&host, "list", json!({"sessionId":"cleanup-a"})).await;
    let file = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "nested/report.txt")
        .unwrap();
    let trash = ok(
        &host,
        "file-action",
        json!({"sessionId":"cleanup-a","path":file["path"],"etag":file["etag"],"action":"trash"}),
    )
    .await;
    assert!(!nested.join("report.txt").exists());
    fs::remove_dir(&nested).unwrap();
    let a_id = a.agent.id().clone();
    a.dispose.await;
    b.dispose.await;
    registry.archive_session(&a_id).await.unwrap();
    registry.unarchive_session(&a_id).await.unwrap();
    registry.archive_session(&a_id).await.unwrap();
    registry.delete_archived_session(&a_id, None).await.unwrap();
    let collected = ok(&host, "collect", json!({"sessionId":"cleanup-a"})).await;
    assert!(
        collected["collected"]
            .as_array()
            .unwrap()
            .contains(&first["id"])
    );
    assert!(
        !collected["collected"]
            .as_array()
            .unwrap()
            .contains(&second["id"])
    );
    assert!(
        !post(&host, "collect", json!({"sessionId":3}))
            .await
            .0
            .is_success()
    );
    ok(
        &host,
        "resource-action",
        json!({"id":first["id"],"action":"restore"}),
    )
    .await;
    ok(
        &host,
        "resource-action",
        json!({"id":first["id"],"action":"restore"}),
    )
    .await;
    ok(
        &host,
        "resource-action",
        json!({"id":trash["id"],"action":"restore-original"}),
    )
    .await;
    assert_eq!(
        fs::read(nested.join("report.txt")).unwrap(),
        b"original report"
    );
    ok(
        &host,
        "resource-action",
        json!({"id":trash["id"],"action":"restore-original"}),
    )
    .await;
    fs::write(nested.join("report.txt"), b"new user content").unwrap();
    assert!(
        !post(
            &host,
            "resource-action",
            json!({"id":trash["id"],"action":"restore-original"})
        )
        .await
        .0
        .is_success()
    );
    assert_eq!(
        fs::read(nested.join("report.txt")).unwrap(),
        b"new user content"
    );
    let learning = ctx
        .get_typed::<Arc<dsh_tool_memory_local::learning::LearningStore>>("learningStore", false)
        .unwrap();
    let entry = learning
        .record_failure(dsh_tool_memory_local::learning::FailureObservation {
            workspace_key: dsh_tool_memory_local::learning::workspace_key(
                &project_b.to_string_lossy(),
            ),
            session_id: "cleanup-b".into(),
            provider: "fixture".into(),
            model: "fixture".into(),
            source: "provider".into(),
            code: "TRANSPORT".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();
    let rows = ok(&host, "resources", json!({"sessionId":"cleanup-b"})).await;
    assert!(
        rows["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == second["id"])
            .unwrap()["protectionReason"]
            .is_string()
    );
    assert!(
        !post(
            &host,
            "resource-action",
            json!({"id":second["id"],"action":"trash"})
        )
        .await
        .0
        .is_success()
    );
    assert!(
        !ok(&host, "collect", json!({})).await["collected"]
            .as_array()
            .unwrap()
            .contains(&second["id"])
    );
    learning
        .invoke(
            "memory.learningRemove",
            json!({"id":entry.id,"expectedRevision":entry.revision}),
        )
        .await
        .unwrap();
    ok(
        &host,
        "resource-action",
        json!({"id":second["id"],"action":"trash"}),
    )
    .await;
    let store_a = dsh_workspace_resources::Store::open(home.join("scratch")).unwrap();
    let project = project_a
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut cache_a = store_a.cache(&project, "fixture").unwrap();
    let cache_id = cache_a.id().to_string();
    cache_a.finish(true).unwrap();
    drop(cache_a);
    let other_root = root.join("other-scratch");
    let settings = ctx
        .get_typed::<Arc<dsh_settings::SettingsProvider>>("settings", false)
        .unwrap();
    settings
        .update(
            &dsh_settings::settings_namespace("workspace-scratch").unwrap(),
            json!({"location":other_root}),
            None,
        )
        .await
        .unwrap();
    ok(&host, "resources", json!({})).await;
    let store_b = dsh_workspace_resources::Store::open(&other_root).unwrap();
    let mut cache_b = store_b.cache(&project, "fixture").unwrap();
    assert_eq!(cache_b.id(), cache_id);
    cache_b.finish(true).unwrap();
    drop(cache_b);
    let rows = ok(&host, "resources", json!({})).await;
    let caches = rows["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["id"] == cache_id)
        .collect::<Vec<_>>();
    assert_eq!(caches.len(), 2);
    assert_ne!(caches[0]["locationId"], caches[1]["locationId"]);
    assert!(
        !post(
            &host,
            "resource-action",
            json!({"id":cache_id,"action":"trash"})
        )
        .await
        .0
        .is_success()
    );
    ok(
        &host,
        "resource-action",
        json!({"id":cache_id,"locationId":caches[0]["locationId"],"action":"trash"}),
    )
    .await;
    let old = home.join("scratch");
    let offline = root.join("offline-store");
    let boundary = root.canonicalize().unwrap();
    assert!(old.canonicalize().unwrap().starts_with(&boundary));
    assert!(
        offline
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .starts_with(&boundary)
    );
    fs::rename(&old, &offline).unwrap();
    let unavailable = ok(&host, "resources", json!({})).await;
    assert!(unavailable["warning"].is_string());
    assert!(!old.exists());
    assert!(offline.canonicalize().unwrap().starts_with(&boundary));
    assert!(
        old.parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .starts_with(&boundary)
    );
    fs::rename(&offline, &old).unwrap();
    let online = ok(&host, "resources", json!({})).await;
    assert_eq!(
        online["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["id"] == cache_id)
            .count(),
        2
    );
    host.shutdown().await.unwrap();
    drop(store_a);
    drop(store_b);
    drop(host);
    drop(learning);
    drop(settings);
    drop(registry);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    fs::remove_dir_all(root).unwrap();
}
