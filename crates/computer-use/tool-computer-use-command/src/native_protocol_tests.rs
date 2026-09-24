use super::*;
use dsh_agent::{AgentFactory, AgentRegistry, CreateAgentOptions};
use dsh_agent_loop::AgentLoop;
use dsh_session::SessionStore;

struct Store(dsh_attachment::ImageAttachmentLimits);
#[async_trait::async_trait]
impl AttachmentStore for Store {
    fn image_limits(&self) -> &dsh_attachment::ImageAttachmentLimits {
        &self.0
    }
    async fn validate_image(
        &self,
        _: &SaveImageAttachment,
    ) -> Result<(), dsh_attachment::AttachmentError> {
        Ok(())
    }
    async fn save_image(
        &self,
        input: &SaveImageAttachment,
    ) -> Result<dsh_attachment::ImageAttachmentRef, dsh_attachment::AttachmentError> {
        Ok(dsh_attachment::ImageAttachmentRef {
            attachment_id: dsh_attachment::attachment_id("native-fixture"),
            media_type: input.media_type,
            bytes: input.data.len() as u64,
            width: 100,
            height: 80,
            name: input.name.clone(),
        })
    }
    async fn read_image(
        &self,
        _: &dsh_attachment::ImageAttachmentRef,
        _: Option<&dsh_attachment::AttachmentAbort>,
    ) -> Result<dsh_attachment::StoredImageAttachment, dsh_attachment::AttachmentError> {
        Err(dsh_attachment::AttachmentError::new(
            "TEST_ONLY",
            "not used",
        ))
    }
}

struct Permission;
impl ComputerPermissionService for Permission {
    fn authorize(
        &self,
        request: ComputerPermissionRequest,
    ) -> futures::future::BoxFuture<'static, Result<ComputerPermissionLease, AdapterError>> {
        Box::pin(async move {
            Ok(ComputerPermissionLease {
                target: request.target,
                revision: 1,
                valid: Arc::new(|| true),
            })
        })
    }
}
struct Driver {
    revision: AtomicU64,
    fail_type: AtomicBool,
    calls: SyncMutex<Vec<Value>>,
}
#[async_trait::async_trait]
impl ComputerUseAdapter for Driver {
    fn adapter_id(&self) -> &'static str {
        "native-browser"
    }
    fn has_owner_activity(&self, _: &str) -> bool {
        true
    }
    async fn permission_identity(
        &self,
        _: &AdapterRequest,
        _: AbortPredicate,
    ) -> Result<ComputerTargetIdentity, AdapterError> {
        Ok(ComputerTargetIdentity {
            host_id: "host".into(),
            device_id: "browser".into(),
            application_id: "browser.exe".into(),
            application_revision: "v1".into(),
            origin: Some("https://fixture.test".into()),
            target_revision: self.revision.load(Ordering::SeqCst).to_string(),
            label: "fixture".into(),
        })
    }
    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        if signal() {
            return Err(AdapterError::cancelled());
        }
        self.calls.lock().push(request.arguments.clone());
        if request.action == "type" && self.fail_type.load(Ordering::SeqCst) {
            return Err(error("COMPUTER_USE_TEST_FAILURE", "fixture input failed"));
        }
        Ok(AdapterOutput {
            value: json!({"state":{"connected":true,"viewport":{"width":100,"height":80}}}),
            screenshot: if matches!(request.action.as_str(), "start" | "capture") {
                Some(AdapterScreenshot {
                    data: vec![1, 2, 3],
                    media_type: "image/png".into(),
                    name: None,
                })
            } else {
                None
            },
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_batches_bind_frames_identity_and_control_and_stop_on_first_failure() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    dsh_llm::LlmRuntime::install(&ctx);
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    ToolRuntime::install(&ctx, Default::default()).unwrap();
    let loops = AgentLoop::install(&ctx, Default::default()).unwrap();
    let owner = loops
        .create_agent(&ctx, CreateAgentOptions::default())
        .await
        .unwrap();
    let permission: Arc<dyn ComputerPermissionService> = Arc::new(Permission);
    ctx.register_service(permission);
    let driver = Arc::new(Driver {
        revision: AtomicU64::new(1),
        fail_type: AtomicBool::new(false),
        calls: Default::default(),
    });
    let runtime = install_adapter(&ctx, 5000, driver.clone()).unwrap();
    let native = NativeProtocol {
        runtime: runtime.clone(),
        target: "browser".into(),
        owners: Default::default(),
    };
    let active: AbortPredicate = Arc::new(|| false);
    let screenshot = json!({"actions":[{"type":"screenshot"}]});
    let click = json!({"actions":[{"type":"click","x":20,"y":30,"button":"left","keys":["CTRL"]}]});
    assert_eq!(
        native
            .execute(owner.agent.clone(), "a", &click, active.clone())
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_FRAME_REQUIRED"
    );
    assert!(driver.calls.lock().is_empty());
    assert!(
        native
            .execute(owner.agent.clone(), "b", &screenshot, active.clone())
            .await
            .unwrap()
            .screenshot
            .is_some()
    );
    let count = driver.calls.lock().len();
    assert_eq!(
        native
            .execute(
                owner.agent.clone(),
                "c",
                &json!({"actions":[{"type":"click","x":100,"y":1,"button":"left"}]}),
                active.clone()
            )
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_FRAME_COORDINATE"
    );
    assert_eq!(driver.calls.lock().len(), count);
    native
        .execute(owner.agent.clone(), "d", &click, active.clone())
        .await
        .unwrap();
    assert!(
        driver
            .calls
            .lock()
            .iter()
            .any(|args| args["action"] == "click" && args["keys"] == json!(["Control"]))
    );
    runtime
        .execute_for_agent(
            owner.agent.clone(),
            &native.arguments(json!({"action":"move","x":1,"y":2})),
            active.clone(),
        )
        .await
        .unwrap();
    let count = driver.calls.lock().len();
    assert_eq!(
        native
            .execute(owner.agent.clone(), "e", &click, active.clone())
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_FRAME_STALE"
    );
    assert_eq!(driver.calls.lock().len(), count);
    native
        .execute(owner.agent.clone(), "f", &screenshot, active.clone())
        .await
        .unwrap();
    driver.revision.store(2, Ordering::SeqCst);
    assert_eq!(
        native
            .execute(owner.agent.clone(), "g", &click, active.clone())
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_APP_IDENTITY_CHANGED"
    );
    native
        .execute(owner.agent.clone(), "h", &screenshot, active.clone())
        .await
        .unwrap();
    runtime
        .execute_for_human(
            owner.agent.clone(),
            &native.arguments(json!({"action":"takeover"})),
            active.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        native
            .execute(owner.agent.clone(), "i", &click, active.clone())
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_FRAME_STALE"
    );
    runtime
        .execute_for_human(
            owner.agent.clone(),
            &native.arguments(json!({"action":"resume_agent"})),
            active.clone(),
        )
        .await
        .unwrap();
    native
        .execute(owner.agent.clone(), "j", &screenshot, active.clone())
        .await
        .unwrap();
    driver.fail_type.store(true, Ordering::SeqCst);
    let count = driver.calls.lock().len();
    assert_eq!(native.execute(owner.agent.clone(),"k",&json!({"actions":[{"type":"type","text":"test"},{"type":"click","x":1,"y":1,"button":"left"}]}),active.clone()).await.unwrap_err().code,"COMPUTER_USE_TEST_FAILURE");
    assert_eq!(driver.calls.lock().len(), count + 1);
    assert_eq!(
        native
            .execute(owner.agent.clone(), "l", &click, active.clone())
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_FRAME_REQUIRED"
    );
    let count = driver.calls.lock().len();
    assert_eq!(
        native
            .execute(owner.agent.clone(), "m", &screenshot, Arc::new(|| true))
            .await
            .unwrap_err()
            .code,
        "COMPUTER_USE_ABORTED"
    );
    assert_eq!(driver.calls.lock().len(), count);
    driver.fail_type.store(false,Ordering::SeqCst);
    native.execute(owner.agent.clone(),"recovered-frame",&screenshot,active.clone()).await.unwrap();
    native.execute(owner.agent.clone(),"recovered-click",&click,active.clone()).await.unwrap();
    assert!(driver.calls.lock().iter().filter(|args|args["action"]=="click").all(|args|args["observedViewport"]==json!({"width":100,"height":80})));
    let store: Arc<dyn AttachmentStore> = Arc::new(Store(dsh_attachment::ImageAttachmentLimits {
        max_image_bytes: 1_000_000,
        max_images_per_message: 8,
        max_message_image_bytes: 8_000_000,
        max_image_pixels: 1_000_000,
        media_types: vec![ImageMediaType::Png],
    }));
    ctx.register_service(store);
    assert!(install_native_protocol(&ctx, "local").is_err());
    install_native_protocol(&ctx, "browser").unwrap();
    assert_eq!(runtime.native_protocol_status()["ready"], true);
    let tools = ctx.get_typed::<Arc<ToolRuntime>>("tools", false).unwrap();
    let tool = tools.get(TOOL_NAME, Some(owner.agent.scope_key())).unwrap();
    assert_eq!(tool.parameters["title"], "dsh-native-computer-v1");
    let result = tools
        .execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id("native-tool"),
            root_call_id: None,
            name: TOOL_NAME.into(),
            arguments: screenshot,
            agent: Some(owner.agent.clone()),
            parent: None,
            signal: active,
        })
        .await;
    assert!(!result.is_error, "{:?}", result.error);
    assert!(
        result
            .content
            .iter()
            .any(|part| matches!(part, dsh_llm::ContentBlock::Image { .. }))
    );
    runtime.shutdown().await.unwrap();
    owner.dispose.await;
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
}
