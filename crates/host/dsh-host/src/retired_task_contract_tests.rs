use super::*;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_project_tasks_preserve_files_and_unrelated_tools() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = workspace.join("PROJECT_TASKS.md");
    let original = "# Personal project notes\n\n<!-- dsh-project-tasks:start -->\n- [ ] [P1] [todo] Keep this task <!-- task:keep -->\n<!-- dsh-project-tasks:end -->\n";
    std::fs::write(&path, original).unwrap();
    let home = root.path().join("host");
    let fixture_preset = home.join(".agent-presets/retirement-fixture");
    std::fs::create_dir_all(&fixture_preset).unwrap();
    for name in ["preset.yml", "agent.cordis.yml"] {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/agent-presets/standard")
            .join(name);
        std::fs::copy(source, fixture_preset.join(name)).unwrap();
    }
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &home, Some("web")).unwrap();
    let registry = ctx
        .get_typed::<Arc<dsh_workspace::WorkspaceRegistry>>("workspaceRegistry", false)
        .unwrap();
    let registered = registry
        .create(&workspace.to_string_lossy(), None)
        .await
        .unwrap();
    let scope_key = dsh_scope::ScopeKey::new();
    let scope = dsh_scope::create_scope(&ctx, scope_key.clone(), &Default::default());
    host.agent_presets
        .mount(&scope.ctx, Some("retirement-fixture"))
        .await
        .unwrap();
    assert!(host.tools.get("project_tasks", Some(&scope_key)).is_none());
    for name in [
        "todo_write",
        "agent_team",
        "job_list",
        "job_output",
        "job_kill",
        "exit_plan_mode",
    ] {
        assert!(
            host.tools.get(name, Some(&scope_key)).is_some(),
            "missing {name}"
        );
    }
    let mut assembly = dsh_system_prompt::AssembleContext::default();
    assembly.scope = Some(scope_key);
    assembly.fields.insert("cwd".into(), json!(workspace));
    let prompt = host.system_prompt.assemble(&ctx, &assembly).await.unwrap();
    assert!(
        !prompt
            .contexts
            .iter()
            .any(|context| context.name == "project:tasks")
    );
    assert!(
        !prompt
            .contexts
            .iter()
            .any(|context| context.text.contains("Keep this task"))
    );

    let base = format!("http://127.0.0.1:{}", host.web_server.port());
    let client = reqwest::Client::new();
    for file_exists in [true, false] {
        if !file_exists {
            std::fs::remove_file(&path).unwrap();
        }
        for action in ["list", "save"] {
            let response = client
                .post(format!("{base}/__dsh-productivity/tasks/{action}"))
                .header("Origin", &base)
                .json(&json!({"workspaceId":registered.id(),"revision":"old","tasks":[]}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 410);
            let value: Value = response.json().await.unwrap();
            assert_eq!(value["code"], "feature-retired");
            assert_eq!(value["feature"], "project-tasks");
            assert_eq!(path.exists(), file_exists);
            if file_exists {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            }
        }
    }
    let response = client
        .post(format!("{base}/__dsh-productivity/tasks/save"))
        .header("Origin", "https://untrusted.invalid")
        .json(&json!({"workspaceId":registered.id(),"tasks":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);
    (scope.dispose)().await;
    host.shutdown().await.unwrap();
}

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
