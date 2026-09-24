use super::super::idle_retirement_tests::control_admission_fixture;
use super::*;
use base64::Engine;
use futures::StreamExt;
use serde_json::{Value, json};

struct Recorder(Arc<Mutex<Vec<String>>>);
impl dsh_llm::LlmAdapter for Recorder {
    fn stream(&self, options: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        self.0.lock().extend(
            options
                .messages
                .iter()
                .flat_map(|message| message.content.iter())
                .map(|block| match block {
                    dsh_llm::ContentBlock::Text { text } => text.clone(),
                    _ => panic!("provider received an unresolved file"),
                }),
        );
        Box::pin(futures::stream::iter([dsh_llm::StreamChunk::Finish {
            reason: dsh_llm::FinishReason::Stop,
            replay_state: None,
        }]))
    }
}

async fn resolve(
    service: &Arc<ApiProxyService>,
    session: &SessionId,
    reference: &FileAttachmentRef,
) -> Value {
    let response = crate::fetch::handler::to_fetch_handler(service.clone()).handle(crate::fetch::handler::CarrierRequest {
        method:http::Method::POST, path:"/api/session.fileAttachment".into(), query:vec![],
        headers:vec![("content-type".into(),"application/json".into())],
        body:Some(serde_json::to_vec(&json!({"type":"client-request","rpcId":"file-test","method":"session.fileAttachment","payload":{"sessionId":session,"attachment":reference}})).unwrap()),
    }).await;
    let Body::Bytes(bytes) = response.into_body() else {
        panic!("unary response required")
    };
    serde_json::from_slice::<Value>(&bytes).unwrap()["result"].clone()
}

#[tokio::test]
async fn admitted_files_reach_the_provider_as_handles_and_only_their_session_can_resolve_them() {
    let (service, agent, detach) = control_admission_fixture("file-owner").await;
    let root = std::env::temp_dir().join(format!("dsh-file-pipeline-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let store = dsh_attachment_local::LocalAttachmentStore::install(
        &service.ctx,
        dsh_attachment_local::Config {
            dsh_home: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    let bytes = b"verbatim input\0\xff";
    let parts = vec![crate::api::sessions::PromptContentPart::File {
        name: "输入资料.docx".into(),
        media_type: None,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }];
    let prepared = crate::prompt_files::prepare(&parts).unwrap();
    let reference = crate::prompt_files::save_reference(&prepared[0], store.as_ref(), None)
        .await
        .unwrap();
    let path = store.file_host_path(&reference).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::File {
            attachment: reference.clone(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    agent
        .session()
        .append(
            "user/message",
            json!(message),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .unwrap();
    let resolved = resolve(&service, agent.id(), &reference).await;
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(resolved["value"]["path"], path.to_string_lossy().as_ref());
    let stranger = service
        .sessions()
        .unwrap()
        .create(
            &service.ctx,
            Some(dsh_session::session_id("unrelated-file-owner")),
            None,
        )
        .await
        .unwrap();
    stranger
        .append(
            "tool/call",
            json!({"arguments":{"type":"file","attachment":reference}}),
            None,
        )
        .unwrap();
    assert_eq!(
        resolve(&service, stranger.id(), &reference).await["error"]["details"]["reason"],
        "ATTACHMENT_NOT_REFERENCED"
    );
    assert!(
        service
            .authorized_attachment_path(stranger.id(), &path, None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service
            .authorized_attachment_path(agent.id(), &path, None)
            .await
            .unwrap(),
        Some(path.clone())
    );

    let policy = dsh_sandbox_policy::SandboxPolicyService::install(
        &service.ctx,
        dsh_sandbox_policy::Config {
            mode: Some(dsh_sandbox::SandboxMode::ReadOnly),
            workspace_root: Some(root.to_string_lossy().into_owned()),
        },
    );
    let allowed = policy.resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
        session: Some(Arc::new(agent.session().clone())),
        mode: None,
    });
    assert_eq!(
        allowed.read_only_roots,
        [path.to_string_lossy().into_owned()]
    );
    assert!(
        policy
            .resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
                session: Some(Arc::new(stranger)),
                mode: None
            })
            .read_only_roots
            .is_empty()
    );

    let runtime = dsh_llm::LlmRuntime::install(&service.ctx);
    let recorded = Arc::new(Mutex::new(vec![]));
    let _binding = runtime
        .register_adapter(
            &service.ctx,
            vec!["files".into()],
            Arc::new(Recorder(recorded.clone())),
        )
        .unwrap();
    let options = dsh_llm::GenerateOptions {
        provider: "files".into(),
        model: "fixture".into(),
        messages: vec![message.clone()],
        reasoning_effort: None,
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: Some(agent.id().to_string()),
        purpose: None,
        agent_loop_request: false,
        telemetry: None,
    };
    let output = runtime.stream(options).collect::<Vec<_>>().await;
    assert!(
        matches!(
            output.as_slice(),
            [dsh_llm::StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                ..
            }]
        ),
        "{output:?}"
    );
    assert_eq!(recorded.lock().len(), 1);
    assert!(recorded.lock()[0].contains(path.to_string_lossy().as_ref()));
    assert!(recorded.lock()[0].contains("read-only input"));
    assert!(matches!(
        message.content[0],
        dsh_llm::ContentBlock::File { .. }
    ));
    assert_eq!(
        agent.session().events()[0].data["content"][0]["type"],
        "file"
    );
    detach().await;
    #[cfg(windows)]
    {
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
