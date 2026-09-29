use super::*;
use dsh_agent::{Agent, AgentFactory, CreateAgentOptions};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone)]
enum Answer {
    Allow,
    Deny,
    Change(PathBuf, Vec<u8>),
    Pending(Arc<tokio::sync::Notify>),
}

async fn call(
    host: &crate::HostSpine,
    owner: &Arc<dyn Agent>,
    name: &str,
    args: Value,
    signal: dsh_tools::AbortPredicate,
) -> Arc<dsh_tools::ToolExecutionResult> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        host.tools.execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: name.into(),
            arguments: args,
            agent: Some(owner.clone()),
            parent: None,
            signal,
        }),
    )
    .await
    .expect("file operation must settle")
}
async fn manage(
    host: &crate::HostSpine,
    owner: &Arc<dyn Agent>,
    args: Value,
) -> Arc<dsh_tools::ToolExecutionResult> {
    call(host, owner, "file_manage", args, Arc::new(|| false)).await
}
async fn api(host: &crate::HostSpine, operation: &str, args: Value) -> Value {
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
    let body: Value = response.json().await.unwrap();
    assert!(status.is_success(), "{operation}: {status} {body}");
    body
}
async fn scratch(host: &crate::HostSpine, owner: &Arc<dyn Agent>) -> PathBuf {
    let allocated = call(
        host,
        owner,
        "workspace_scratch",
        json!({"action":"allocate","kind":"script","label":"file operation fixture"}),
        Arc::new(|| false),
    )
    .await;
    assert!(!allocated.is_error, "{:?}", allocated.error);
    let value = allocated.value.as_ref().unwrap();
    let write = call(
        host,
        owner,
        "workspace_scratch",
        json!({"action":"write","id":value["id"],"path":"owned.txt","content":"owned scratch"}),
        Arc::new(|| false),
    )
    .await;
    assert!(!write.is_error, "{:?}", write.error);
    PathBuf::from(value["path"].as_str().unwrap()).join("owned.txt")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_manage_real_runtime_approvals_recovery_scope_and_stale_inputs() {
    let root = std::env::temp_dir().join(format!("file-operations-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    let workspace = root.join("项目 空格");
    let outside = root.join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let ctx = cordis::Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &home, None).unwrap();
    let make = |id: &str| CreateAgentOptions {
        session_id: Some(dsh_session::session_id(id)),
        meta: Some(dsh_session::CreateSessionMeta {
            cwd: Some(workspace.to_string_lossy().into_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = host
        .agent_loop
        .create_agent(&ctx, make("file-owner-a"))
        .await
        .unwrap();
    let b = host
        .agent_loop
        .create_agent(&ctx, make("file-owner-b"))
        .await
        .unwrap();
    let owner = a.agent.clone();
    for agent in [&a.agent, &b.agent] {
        agent
            .session()
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        dsh_user_approval::set_approval_policy(
            agent.session(),
            dsh_user_approval::ApprovalPolicy::Ask,
        )
        .unwrap();
    }
    let answer = Arc::new(parking_lot::Mutex::new(Answer::Deny));
    let answer_for_hook = answer.clone();
    let asks = Arc::new(AtomicUsize::new(0));
    let asks_for_hook = asks.clone();
    let human = ctx
        .on(
            "approval/human-request",
            Arc::new(move |_, args| {
                asks_for_hook.fetch_add(1, Ordering::SeqCst);
                let request = cordis::downcast_arc::<ApprovalRequest>(&args[0]).unwrap();
                assert_eq!(request.tool_name, "file_manage");
                assert!(!request.rememberable);
                assert!(request.grant_key.is_none());
                let answer = answer_for_hook.lock().clone();
                Box::pin(async move {
                    Some(cordis::arc(match answer {
                        Answer::Allow => ApprovalOutcome::AllowedOnce,
                        Answer::Deny => ApprovalOutcome::Rejected,
                        Answer::Change(path, bytes) => {
                            std::fs::write(path, bytes).unwrap();
                            ApprovalOutcome::AllowedOnce
                        }
                        Answer::Pending(entered) => {
                            entered.notify_one();
                            std::future::pending::<ApprovalOutcome>().await
                        }
                    }))
                })
            }),
            cordis::EventOptions::default().prepend(true).global(true),
        )
        .await;

    let file = workspace.join("正式 电话.txt");
    std::fs::write(&file, b"0013800012345\n7 rows").unwrap();
    let denied = manage(&host, &owner, json!({"action":"delete","file_path":file})).await;
    assert!(denied.is_error);
    assert_eq!(
        denied.meta.as_ref().unwrap()["executionReceipt"]["effects"],
        "none"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"0013800012345\n7 rows");
    assert_eq!(asks.load(Ordering::SeqCst), 1);
    *answer.lock() = Answer::Allow;
    let deleted = manage(&host, &owner, json!({"action":"delete","file_path":file})).await;
    assert!(!deleted.is_error, "{:?}", deleted.error);
    let value = deleted.value.as_ref().unwrap();
    assert_eq!(value["removed"], true);
    assert_eq!(value["recoverable"], true);
    assert!(!file.exists());
    let id = value["id"].as_str().unwrap();
    let listing = api(&host, "resources", json!({"sessionId":owner.id()})).await;
    let resource = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap();
    assert_eq!(resource["kind"], "trash");
    assert_eq!(resource["owner"], owner.id().as_str());
    let payload = PathBuf::from(resource["path"].as_str().unwrap()).join("payload");
    assert_eq!(std::fs::read(&payload).unwrap(), b"0013800012345\n7 rows");
    let basefs = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .unwrap()
        .as_ref()
        .clone();
    let fs = basefs.for_tool(Some(owner.id().as_str())).unwrap();
    let target = fs.resolve(&payload.to_string_lossy(), None).await.unwrap();
    assert!(fs.authorize_write(&target).await.is_err());
    assert!(
        fs.write_text(&target, "must not replace recovery bytes", None, None, None)
            .await
            .is_err()
    );
    let before = asks.load(Ordering::SeqCst);
    assert!(
        manage(
            &host,
            &owner,
            json!({"action":"delete","file_path":payload})
        )
        .await
        .is_error
    );
    assert_eq!(asks.load(Ordering::SeqCst), before);
    api(
        &host,
        "resource-action",
        json!({"action":"restore-original","id":id}),
    )
    .await;
    assert_eq!(std::fs::read(&file).unwrap(), b"0013800012345\n7 rows");

    let existing = workspace.join("existing.txt");
    std::fs::write(&existing, b"KEEP").unwrap();
    assert!(
        manage(
            &host,
            &owner,
            json!({"action":"rename","file_path":file,"new_path":existing})
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"KEEP");
    assert!(file.exists());
    assert_eq!(asks.load(Ordering::SeqCst), before);
    let destination = workspace.join("改名 电话.txt");
    let renamed = manage(
        &host,
        &owner,
        json!({"action":"rename","file_path":file,"new_path":destination}),
    )
    .await;
    assert!(!renamed.is_error, "{:?}", renamed.error);
    assert!(!file.exists());
    assert!(destination.exists());
    let collision = workspace.join("late-target.txt");
    *answer.lock() = Answer::Change(collision.clone(), b"late target".to_vec());
    assert!(
        manage(
            &host,
            &owner,
            json!({"action":"rename","file_path":destination,"new_path":collision})
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&collision).unwrap(), b"late target");
    assert!(destination.exists());
    *answer.lock() = Answer::Change(destination.clone(), b"changed by another editor".to_vec());
    assert!(
        manage(
            &host,
            &owner,
            json!({"action":"delete","file_path":destination})
        )
        .await
        .is_error
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"changed by another editor"
    );

    let owned = scratch(&host, &owner).await;
    let foreign = scratch(&host, &b.agent).await;
    let private = home.join("private-fixture.txt");
    std::fs::write(&private, b"synthetic private").unwrap();
    let external = outside.join("foreign.txt");
    std::fs::write(&external, b"outside").unwrap();
    let directory = workspace.join("directory");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("keep.txt"), b"keep").unwrap();
    *answer.lock() = Answer::Allow;
    let before = asks.load(Ordering::SeqCst);
    for forbidden in [&directory, &foreign, &private, &external] {
        assert!(
            manage(
                &host,
                &owner,
                json!({"action":"delete","file_path":forbidden})
            )
            .await
            .is_error,
            "{forbidden:?}"
        );
        assert!(forbidden.exists());
    }
    assert_eq!(
        asks.load(Ordering::SeqCst),
        before,
        "invalid paths must be rejected before prompting"
    );
    let owned_new = owned.with_file_name("renamed-owned.txt");
    assert!(
        !manage(
            &host,
            &owner,
            json!({"action":"rename","file_path":owned,"new_path":owned_new})
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&owned_new).unwrap(), b"owned scratch");

    let entered = Arc::new(tokio::sync::Notify::new());
    *answer.lock() = Answer::Pending(entered.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_request = cancel.clone();
    let tools = host.tools.clone();
    let cancel_owner = owner.clone();
    let cancel_file = destination.clone();
    let pending = tokio::spawn(async move {
        tools
            .execute(dsh_tools::ToolExecutionInput {
                call_id: dsh_llm::call_id("cancel-delete"),
                root_call_id: None,
                name: "file_manage".into(),
                arguments: json!({"action":"delete","file_path":cancel_file}),
                agent: Some(cancel_owner),
                parent: None,
                signal: Arc::new(move || cancel_for_request.load(Ordering::SeqCst)),
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    cancel.store(true, Ordering::SeqCst);
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(cancelled.is_error);
    assert_eq!(
        cancelled.meta.as_ref().unwrap()["executionReceipt"]["effects"],
        "none"
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"changed by another editor"
    );
    assert_eq!(
        owner
            .session()
            .find_event_rev(|event| event.type_ == "approval/decided")
            .unwrap()
            .unwrap()
            .data["outcome"],
        "cancelled"
    );
    human().await;
    for agent in [&a.agent, &b.agent] {
        agent
            .session()
            .append(
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
                None,
            )
            .unwrap();
    }
    a.dispose.await;
    b.dispose.await;
    drop(owner);
    drop(a.agent);
    drop(b.agent);
    drop(fs);
    drop(basefs);
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
