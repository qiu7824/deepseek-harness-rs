use super::*;
use dsh_agent::{AgentFactory, CreateAgentOptions};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn call(
    host: &crate::HostSpine,
    agent: Arc<dyn dsh_agent::Agent>,
    name: &str,
    args: Value,
    cancelled: bool,
) -> Arc<dsh_tools::ToolExecutionResult> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.tools.execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: name.into(),
            arguments: args,
            agent: Some(agent),
            parent: None,
            signal: Arc::new(move || cancelled),
        }),
    )
    .await
    .expect("Office tool must settle")
}
fn workbook(path: &Path, overwrite: bool, phone: &str) -> Value {
    json!({"file_path":path,"format":"xlsx","overwrite":overwrite,"sheets":[{"name":"电话","rows":[["姓名","电话"],["张三",phone]]}]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn office_tools_use_real_fs_without_shell_and_preserve_approval_cancellation_and_attachment_boundaries()
 {
    let root = std::env::temp_dir().join(format!("office-tools-{}", uuid::Uuid::new_v4()));
    let project = root.join("工程 空格");
    let home = root.join("private-home");
    std::fs::create_dir_all(&project).unwrap();
    let ctx = Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &home, None).unwrap();
    let create = |id: &str| CreateAgentOptions {
        session_id: Some(dsh_session::session_id(format!(
            "office-{id}-{}",
            uuid::Uuid::new_v4()
        ))),
        meta: Some(dsh_session::CreateSessionMeta {
            cwd: Some(project.to_string_lossy().into_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = host
        .agent_loop
        .create_agent(&ctx, create("owner"))
        .await
        .unwrap()
        .agent;
    let b = host
        .agent_loop
        .create_agent(&ctx, create("other"))
        .await
        .unwrap()
        .agent;
    dsh_sandbox_policy::set_sandbox_mode(a.session(), dsh_sandbox::SandboxMode::WorkspaceWrite)
        .unwrap();
    dsh_user_approval::set_approval_policy(a.session(), dsh_user_approval::ApprovalPolicy::Ask)
        .unwrap();
    dsh_user_approval::set_approval_policy(b.session(), dsh_user_approval::ApprovalPolicy::Never)
        .unwrap();
    a.session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let approve = Arc::new(AtomicUsize::new(0));
    let decisions = Arc::new(AtomicUsize::new(0));
    let permit = approve.clone();
    let seen = decisions.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_, args| {
        let request = cordis::downcast::<dsh_user_approval::ApprovalRequest>(&args[0]).unwrap();
        assert!(!request.rememberable);
        assert!(request.grant_key.is_none());
        seen.fetch_add(1, Ordering::SeqCst);
        let allow = permit.load(Ordering::SeqCst);
        Box::pin(async move {
            Some(cordis::arc(match allow {
                1 => dsh_user_approval::ApprovalOutcome::AllowedOnce,
                2 => dsh_user_approval::ApprovalOutcome::Cancelled,
                _ => dsh_user_approval::ApprovalOutcome::Rejected,
            }))
        })
    });
    ctx.events.register(
        &ctx,
        "Office explicit human decision",
        "approval/human-request",
        listener,
        &cordis::EventOptions::default().prepend(true).global(true),
    );
    let path = project.join("literal-phones.xlsx");
    let initial = call(
        &host,
        a.clone(),
        "office_write",
        workbook(&path, false, "0013812345678"),
        false,
    )
    .await;
    assert!(!initial.is_error, "{:?}", initial.error);
    assert_eq!(initial.value.as_ref().unwrap()["operation"], "created");
    let original = std::fs::read(&path).unwrap();
    assert!(original.starts_with(b"PK\x03\x04"));
    assert_eq!(decisions.load(Ordering::SeqCst), 0);
    let read = call(
        &host,
        a.clone(),
        "office_read",
        json!({"file_path":path,"sheet_name":"电话","start_row":2,"row_limit":1}),
        false,
    )
    .await;
    assert!(!read.is_error, "{:?}", read.error);
    assert_eq!(
        read.value.as_ref().unwrap()["sheets"][0]["rows"][0]["cells"][1]["rawValue"],
        "0013812345678"
    );
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            workbook(&path, false, "different"),
            false
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            workbook(&path, true, "different"),
            false
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(decisions.load(Ordering::SeqCst), 1);
    approve.store(1, Ordering::SeqCst);
    let replaced = call(
        &host,
        a.clone(),
        "office_write",
        workbook(&path, true, "00001012345678"),
        false,
    )
    .await;
    assert!(!replaced.is_error, "{:?}", replaced.error);
    assert_eq!(
        decisions.load(Ordering::SeqCst),
        2,
        "each approved overwrite prompts exactly once"
    );
    assert_eq!(replaced.value.as_ref().unwrap()["operation"], "replaced");
    let aborted = project.join("cancelled.xlsx");
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            workbook(&aborted, false, "0001"),
            true
        )
        .await
        .is_error
    );
    assert!(!aborted.exists());
    dsh_sandbox_policy::set_sandbox_mode(a.session(), dsh_sandbox::SandboxMode::ReadOnly).unwrap();
    approve.store(0, Ordering::SeqCst);
    let denied = project.join("read-only.xlsx");
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            workbook(&denied, false, "0001"),
            false
        )
        .await
        .is_error
    );
    assert!(!denied.exists());
    approve.store(1, Ordering::SeqCst);
    let single_write = call(
        &host,
        a.clone(),
        "office_write",
        workbook(&denied, false, "001234"),
        false,
    )
    .await;
    assert!(!single_write.is_error, "{:?}", single_write.error);
    assert_eq!(
        ctx.get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
            .unwrap()
            .try_override_of(a.session())
            .unwrap(),
        Some(dsh_sandbox::SandboxMode::ReadOnly)
    );
    approve.store(2, Ordering::SeqCst);
    let withdrawn = project.join("cancelled-during-approval.xlsx");
    let cancelled = call(
        &host,
        a.clone(),
        "office_write",
        workbook(&withdrawn, false, "00123"),
        false,
    )
    .await;
    assert_eq!(
        cancelled
            .error
            .as_ref()
            .unwrap()
            .info
            .as_ref()
            .unwrap()
            .code,
        "USER_APPROVAL_CANCELLED"
    );
    assert!(!withdrawn.exists());
    approve.store(1, Ordering::SeqCst);
    dsh_sandbox_policy::set_sandbox_mode(a.session(), dsh_sandbox::SandboxMode::WorkspaceWrite)
        .unwrap();
    let attachments = ctx
        .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
        .unwrap()
        .as_ref()
        .clone();
    let reference = attachments
        .save_file_stream(
            Box::pin(std::io::Cursor::new(original.clone())),
            "上传电话.xlsx".into(),
            None,
        )
        .await
        .unwrap();
    let attached = attachments.file_host_path(&reference).unwrap();
    a.session().append("user/message",json!({"id":"office-input","role":"user","source":{"kind":"user"},"content":[{"type":"file","attachment":reference}]}),Some(dsh_session::SurfaceIntent{surface_op:dsh_session::SurfaceOp::Append,source_event_seqs:None})).unwrap();
    assert!(
        !call(
            &host,
            a.clone(),
            "office_read",
            json!({"file_path":attached}),
            false
        )
        .await
        .is_error
    );
    assert!(
        call(
            &host,
            b.clone(),
            "office_read",
            json!({"file_path":attached}),
            false
        )
        .await
        .is_error
    );
    let private = home.join("secret.xlsx");
    std::fs::write(&private, &original).unwrap();
    assert!(
        call(
            &host,
            a.clone(),
            "office_read",
            json!({"file_path":private}),
            false
        )
        .await
        .is_error
    );
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            workbook(&private, true, "not allowed"),
            false
        )
        .await
        .is_error
    );
    assert_eq!(std::fs::read(&private).unwrap(), original);
    let unsupported = project.join("old.doc");
    assert!(
        call(
            &host,
            a.clone(),
            "office_write",
            json!({"file_path":unsupported,"format":"docx","paragraphs":["text"]}),
            false
        )
        .await
        .is_error
    );
    assert!(!unsupported.exists());
    a.session()
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    host.shutdown().await.unwrap();
    drop(attachments);
    drop(a);
    drop(b);
    drop(host);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
