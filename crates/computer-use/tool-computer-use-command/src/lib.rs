//! Model-facing Computer Use tool with a built-in isolated Chromium/Edge
//! controller and a backwards-compatible external-command adapter.

mod adapter;
mod browser;
mod command;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_attachment::{AttachmentStore, ImageMediaType, SaveImageAttachment};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};
use parking_lot::Mutex as SyncMutex;
use serde_json::{Value, json};

pub use adapter::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, AdapterScreenshot,
    ComputerUseAdapter,
};
pub use browser::{NativeBrowserAdapter, NativeBrowserConfig, discover_browser_executable};
pub use command::CommandAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterMode {
    /// Use the configured command when one exists; otherwise use the built-in
    /// isolated browser. This preserves pre-native-browser installations.
    Auto,
    Command,
    NativeBrowser,
}

impl AdapterMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "" | "auto" => Ok(Self::Auto),
            "command" => Ok(Self::Command),
            "native-browser" => Ok(Self::NativeBrowser),
            other => Err(format!(
                "unsupported computer-use adapter {other:?}; expected auto, command or native-browser"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub adapter: AdapterMode,
    pub command: String,
    pub timeout_ms: u64,
    pub browser_executable: Option<PathBuf>,
    pub browser_data_root: PathBuf,
    pub browser_headless: bool,
    pub max_browser_sessions: usize,
}

/// Shared runtime used by both the model tool and the local GUI route. The
/// owner id is supplied by trusted Host code and cannot be overridden by tool
/// arguments, so browser sessions with the same display name stay isolated
/// between conversations.
pub struct ComputerUseRuntime {
    ctx: Context,
    adapter: Arc<dyn ComputerUseAdapter>,
    timeout: Duration,
    owner_agents: SyncMutex<HashMap<String, Weak<dyn dsh_agent::Agent>>>,
}

impl cordis::Service for ComputerUseRuntime {
    fn service_name(&self) -> &'static str {
        "computerUse"
    }
}

impl ComputerUseRuntime {
    pub fn adapter_id(&self) -> &'static str {
        self.adapter.adapter_id()
    }

    pub fn availability(&self) -> Result<(), AdapterError> {
        self.adapter.availability()
    }

    pub async fn execute(
        &self,
        owner_id: impl Into<String>,
        arguments: &Value,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        self.execute_inner(owner_id.into(), None, arguments, signal)
            .await
    }

    pub async fn execute_for_agent(
        &self,
        owner: Arc<dyn dsh_agent::Agent>,
        arguments: &Value,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        self.owner_agents
            .lock()
            .insert(owner.id().as_str().to_string(), Arc::downgrade(&owner));
        self.execute_inner(
            owner.id().as_str().to_string(),
            Some(owner),
            arguments,
            signal,
        )
        .await
    }

    async fn execute_inner(
        &self,
        owner_id: String,
        owner: Option<Arc<dyn dsh_agent::Agent>>,
        arguments: &Value,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        let was_active = self.adapter.has_owner_activity(&owner_id);
        let request = AdapterRequest::from_arguments(arguments)?.with_owner_id(owner_id.clone());
        let result = tokio::select! {
            result = self.adapter.execute(request, Arc::clone(&signal)) => result,
            _ = wait_for_cancel(Arc::clone(&signal)) => Err(AdapterError::cancelled()),
            _ = tokio::time::sleep(self.timeout) => Err(AdapterError::new(
                "COMPUTER_USE_TIMEOUT",
                format!("computer-use action exceeded {} ms", self.timeout.as_millis()),
            )),
        };
        let is_active = self.adapter.has_owner_activity(&owner_id);
        if was_active
            && !is_active
            && let Some(owner) = owner
        {
            self.ctx
                .emit("computer-use/owner-idle", vec![cordis::arc(owner)]);
        }
        if !is_active {
            self.owner_agents.lock().remove(&owner_id);
        }
        let mut output = result?;
        if signal() {
            return Err(AdapterError::cancelled());
        }
        let object = output.value.as_object_mut().ok_or_else(|| {
            AdapterError::new(
                "COMPUTER_USE_INVALID_OUTPUT",
                "computer-use adapter output must be a JSON object",
            )
        })?;
        object
            .entry("adapter".to_string())
            .or_insert_with(|| Value::String(self.adapter.adapter_id().to_string()));
        Ok(output)
    }

    pub async fn shutdown(&self) -> Result<(), AdapterError> {
        let result = self.adapter.shutdown().await;
        self.owner_agents.lock().clear();
        result
    }

    pub async fn close_owner(&self, owner_id: &str) -> Result<(), AdapterError> {
        let result = self.adapter.close_owner(owner_id).await;
        if !self.adapter.has_owner_activity(owner_id) {
            self.owner_agents.lock().remove(owner_id);
        }
        result
    }

    pub fn has_owner_activity(&self, owner: &Arc<dyn dsh_agent::Agent>) -> bool {
        self.adapter.has_owner_activity(owner.id().as_str())
    }

    async fn reap_inactive(&self) {
        for owner_id in self.adapter.reap_inactive().await {
            if self.adapter.has_owner_activity(&owner_id) {
                continue;
            }
            let owner = self
                .owner_agents
                .lock()
                .remove(&owner_id)
                .and_then(|owner| owner.upgrade());
            if let Some(owner) = owner {
                self.ctx
                    .emit("computer-use/owner-idle", vec![cordis::arc(owner)]);
            }
        }
    }
}

const READ_ONLY_ACTIONS: &[&str] = &[
    "capture",
    "status",
    "cua_browser_state",
    "list_sessions",
    "list_apps",
    "list_windows",
];

pub fn action_requires_approval(action: &str) -> bool {
    !READ_ONLY_ACTIONS.contains(&action)
}

pub fn install(ctx: &Context, config: Config) -> Result<Arc<ComputerUseRuntime>, String> {
    if config.timeout_ms < 1_000 || config.timeout_ms > 300_000 {
        return Err("computer-use timeout must be between 1000 and 300000 ms".to_string());
    }
    let mode = match config.adapter {
        AdapterMode::Auto if !config.command.trim().is_empty() => AdapterMode::Command,
        AdapterMode::Auto => AdapterMode::NativeBrowser,
        mode => mode,
    };
    let adapter: Arc<dyn ComputerUseAdapter> = match mode {
        AdapterMode::Command => Arc::new(
            CommandAdapter::with_timeout(config.command, Duration::from_millis(config.timeout_ms))
                .map_err(|error| error.message)?,
        ),
        AdapterMode::NativeBrowser => Arc::new(
            NativeBrowserAdapter::new(NativeBrowserConfig {
                executable: config.browser_executable,
                data_root: config.browser_data_root,
                headless: config.browser_headless,
                max_sessions: config.max_browser_sessions,
                launch_timeout: Duration::from_millis(config.timeout_ms.min(15_000)),
                action_timeout: Duration::from_millis(config.timeout_ms),
                ..NativeBrowserConfig::default()
            })
            .map_err(|error| error.message)?,
        ),
        AdapterMode::Auto => unreachable!("auto mode is resolved above"),
    };
    install_adapter(ctx, config.timeout_ms, adapter)
}

/// Register a concrete adapter. This is also the stable integration seam for
/// a future remote-desktop or UU controller: it must implement the same
/// cancellation and screenshot contract rather than impersonating an iframe.
pub fn install_adapter(
    ctx: &Context,
    timeout_ms: u64,
    adapter: Arc<dyn ComputerUseAdapter>,
) -> Result<Arc<ComputerUseRuntime>, String> {
    if timeout_ms < 1_000 || timeout_ms > 300_000 {
        return Err("computer-use timeout must be between 1000 and 300000 ms".to_string());
    }
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "computer-use requires the tools service".to_string())?;
    let attachments = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .map(|slot| slot.as_ref().clone());

    let listener: Arc<Listener> = Arc::new(|_ctx, args| {
        let execution = args
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|slot| slot.as_ref().clone());
        let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
        Box::pin(async move {
            if let Some(execution) = execution
                && execution.name == "computer_use"
                && action_requires_approval(
                    execution
                        .arguments
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            {
                return Some(arc(PreToolDecision::Ask {
                    reason: Some("Computer Use 将操作隔离浏览器或桌面，需要用户确认".to_string()),
                    grant_key: Some("tool:computer_use".to_string()),
                    rememberable: true,
                }));
            }
            let Some(next) = next else {
                return Some(arc(PreToolDecision::Allow));
            };
            Some(next.call().await)
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    ));

    let runtime = Arc::new(ComputerUseRuntime {
        ctx: ctx.clone(),
        adapter,
        timeout: Duration::from_millis(timeout_ms),
        owner_agents: SyncMutex::new(HashMap::new()),
    });
    ctx.register_service(Arc::clone(&runtime));
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let runtime_for_reaper = Arc::downgrade(&runtime);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_for_task = Arc::clone(&stopped);
        handle.spawn(async move {
            while !stopped_for_task.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let Some(runtime) = runtime_for_reaper.upgrade() else {
                    return;
                };
                runtime.reap_inactive().await;
            }
        });
        let _ = ctx.effect(
            "computerUse.reaper",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    stopped.store(true, Ordering::SeqCst);
                    Box::pin(async {})
                }))
            }),
        );
    }
    let runtime_for_shutdown = Arc::downgrade(&runtime);
    let _ = ctx.effect(
        "computerUse.shutdown",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                let runtime = runtime_for_shutdown.clone();
                Box::pin(async move {
                    if let Some(runtime) = runtime.upgrade() {
                        let _ = runtime.shutdown().await;
                    }
                })
            }))
        }),
    );
    let runtime_for_disposed = Arc::downgrade(&runtime);
    let disposed_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let runtime = runtime_for_disposed.clone();
        let owner_id = args
            .first()
            .and_then(|value| cordis::downcast::<dsh_agent::AgentLifecyclePayload>(value))
            .map(|payload| payload.agent.id().as_str().to_string());
        Box::pin(async move {
            if let (Some(runtime), Some(owner_id)) = (runtime.upgrade(), owner_id) {
                tokio::spawn(async move {
                    let _ = runtime.close_owner(&owner_id).await;
                });
            }
            None
        })
    });
    futures::executor::block_on(ctx.on(
        "agent/disposed",
        disposed_listener,
        EventOptions::default().global(true),
    ));
    let runtime_for_session_disposed = Arc::downgrade(&runtime);
    let session_disposed_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let runtime = runtime_for_session_disposed.clone();
        let owner_id = args
            .first()
            .and_then(|value| cordis::downcast::<dsh_session::Session>(value))
            .map(|session| session.id().as_str().to_string());
        Box::pin(async move {
            if let (Some(runtime), Some(owner_id)) = (runtime.upgrade(), owner_id) {
                tokio::spawn(async move {
                    let _ = runtime.close_owner(&owner_id).await;
                });
            }
            None
        })
    });
    futures::executor::block_on(ctx.on(
        "session/disposed",
        session_disposed_listener,
        EventOptions::default().global(true),
    ));
    let runtime_for_session_deleted = Arc::downgrade(&runtime);
    let session_deleted_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let runtime = runtime_for_session_deleted.clone();
        let owner_id = args
            .first()
            .and_then(|value| cordis::downcast::<dsh_session::SessionId>(value))
            .map(|session_id| session_id.as_str().to_string());
        Box::pin(async move {
            if let (Some(runtime), Some(owner_id)) = (runtime.upgrade(), owner_id) {
                tokio::spawn(async move {
                    let _ = runtime.close_owner(&owner_id).await;
                });
            }
            None
        })
    });
    futures::executor::block_on(ctx.on(
        "workspace/session-deleted",
        session_deleted_listener,
        EventOptions::default().global(true),
    ));
    let runtime_for_execute = Arc::clone(&runtime);
    tools.register(
        ctx,
        ToolDefinition {
            name: "computer_use".to_string(),
            description: "Operate a persistent, isolated browser or configured desktop adapter. Reuse sessionId across calls. Start or navigate, inspect the returned state and screenshot, perform one or more click/type/scroll actions, then inspect the new screenshot. Mutating actions require user approval and all actions support cancellation and timeout.".to_string(),
            parameters: json!({
                "type": "object",
                "additionalProperties": true,
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Built-in browser actions: start, status, capture, navigate, click, double_click, type, scroll, list_sessions, close. Command adapters may add actions."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Stable isolated browser-session name. Defaults to default."
                    },
                    "url": { "type": "string", "description": "Absolute http/https URL for start or navigate." },
                    "x": { "type": "number", "description": "Viewport x coordinate; validated between 0 and 100000." },
                    "y": { "type": "number", "description": "Viewport y coordinate; validated between 0 and 100000." },
                    "button": { "type": "string", "enum": ["left", "right", "middle", "back", "forward"] },
                    "text": { "type": "string", "description": "Text inserted into the focused element; type may also include x and y to focus first." },
                    "deltaX": { "type": "number" },
                    "deltaY": { "type": "number" },
                    "waitMs": { "type": "integer", "description": "Optional settle delay from 0 through 10000 milliseconds." },
                    "includeScreenshot": { "type": "boolean" }
                },
                "required": ["action"]
            }),
            output: ToolOutputDefinition {
                schema: json!({}),
                render: Arc::new(|_args, value| render_output(value)),
                presentation_meta: None,
            },
            timeout_ms: Some(timeout_ms),
            is_concurrency_safe: Some(Arc::new(|args| {
                !action_requires_approval(
                    args.get("action").and_then(Value::as_str).unwrap_or_default(),
                )
            })),
            execute: Arc::new(move |args, run: &ToolRunContext| {
                let runtime = Arc::clone(&runtime_for_execute);
                let attachments = attachments.clone();
                let arguments = args.clone();
                let signal = run.signal.lock().clone();
                let owner = run.agent.clone();
                Box::pin(async move {
                    let mut output = match owner {
                        Some(owner) => runtime
                            .execute_for_agent(owner, &arguments, Arc::clone(&signal))
                            .await,
                        None => runtime
                            .execute("host", &arguments, Arc::clone(&signal))
                            .await,
                    }
                    .map_err(tool_body_error)?;
                    if signal() {
                        return Err(tool_body_error(AdapterError::cancelled()));
                    }
                    let object = output
                        .value
                        .as_object_mut()
                        .expect("ComputerUseRuntime validates object outputs");
                    if let Some(screenshot) = output.screenshot.take() {
                        let attachments = attachments.ok_or_else(|| {
                            ToolBodyError::coded(
                                "computer-use screenshot requires the attachments service",
                                "ComputerUseError",
                                "COMPUTER_USE_ATTACHMENTS_REQUIRED",
                            )
                        })?;
                        let media_type = image_media_type(&screenshot.media_type)
                            .map_err(tool_body_error)?;
                        let saved = attachments
                            .save_image(&SaveImageAttachment {
                                data: screenshot.data,
                                media_type,
                                name: screenshot.name,
                            })
                            .await
                            .map_err(|error| {
                                ToolBodyError::coded(
                                    format!("computer-use screenshot was rejected: {error}"),
                                    "ComputerUseError",
                                    "COMPUTER_USE_SCREENSHOT_REJECTED",
                                )
                            })?;
                        object.insert(
                            "screenshot".to_string(),
                            serde_json::to_value(saved).map_err(|error| {
                                ToolBodyError::coded(
                                    error.to_string(),
                                    "ComputerUseError",
                                    "COMPUTER_USE_SCREENSHOT_SERIALIZE",
                                )
                            })?,
                        );
                    }
                    Ok(output.value)
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        },
    )?;
    Ok(runtime)
}

fn tool_body_error(error: AdapterError) -> ToolBodyError {
    ToolBodyError::coded(error.message, "ComputerUseError", &error.code)
}

async fn wait_for_cancel(signal: AbortPredicate) {
    loop {
        if signal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn image_media_type(value: &str) -> Result<ImageMediaType, AdapterError> {
    match value {
        "image/png" => Ok(ImageMediaType::Png),
        "image/jpeg" => Ok(ImageMediaType::Jpeg),
        "image/webp" => Ok(ImageMediaType::Webp),
        "image/gif" => Ok(ImageMediaType::Gif),
        other => Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TYPE",
            format!("unsupported screenshot media type: {other}"),
        )),
    }
}

fn render_output(value: &Value) -> Result<Vec<dsh_llm::ContentBlock>, String> {
    let mut blocks = vec![dsh_llm::ContentBlock::Text {
        text: serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()),
    }];
    if let Some(screenshot) = value.get("screenshot") {
        let attachment = serde_json::from_value::<dsh_llm::ImageAttachmentRef>(screenshot.clone())
            .map_err(|error| format!("invalid computer-use screenshot reference: {error}"))?;
        blocks.push(dsh_llm::ContentBlock::Image { attachment });
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_attachment::{
        AttachmentAbort, AttachmentError, ImageAttachmentLimits, ImageAttachmentRef,
        StoredImageAttachment, attachment_id,
    };
    use dsh_tools::ToolExecutionInput;

    struct TestAttachmentStore {
        limits: ImageAttachmentLimits,
    }

    #[async_trait::async_trait]
    impl AttachmentStore for TestAttachmentStore {
        fn image_limits(&self) -> &ImageAttachmentLimits {
            &self.limits
        }

        async fn validate_image(
            &self,
            _input: &SaveImageAttachment,
        ) -> Result<(), AttachmentError> {
            Ok(())
        }

        async fn save_image(
            &self,
            input: &SaveImageAttachment,
        ) -> Result<ImageAttachmentRef, AttachmentError> {
            Ok(ImageAttachmentRef {
                attachment_id: attachment_id("sha256:computer-use-test"),
                media_type: input.media_type,
                bytes: input.data.len() as u64,
                width: 1280,
                height: 720,
                name: input.name.clone(),
            })
        }

        async fn read_image(
            &self,
            _reference: &ImageAttachmentRef,
            _signal: Option<&AttachmentAbort>,
        ) -> Result<StoredImageAttachment, AttachmentError> {
            Err(AttachmentError::new("TEST_ONLY", "not used"))
        }
    }

    struct TestAdapter;

    #[async_trait::async_trait]
    impl ComputerUseAdapter for TestAdapter {
        fn adapter_id(&self) -> &'static str {
            "test-browser"
        }

        async fn execute(
            &self,
            request: AdapterRequest,
            _signal: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            Ok(AdapterOutput {
                value: json!({
                    "ok": true,
                    "action": request.action,
                    "state": {"url":"https://example.test/","title":"fixture"}
                }),
                screenshot: Some(AdapterScreenshot {
                    data: b"\x89PNG\r\n\x1a\n".to_vec(),
                    media_type: "image/png".to_string(),
                    name: Some("fixture.png".to_string()),
                }),
            })
        }
    }

    struct SlowAdapter;

    #[async_trait::async_trait]
    impl ComputerUseAdapter for SlowAdapter {
        fn adapter_id(&self) -> &'static str {
            "slow"
        }

        async fn execute(
            &self,
            _request: AdapterRequest,
            _signal: AbortPredicate,
        ) -> Result<AdapterOutput, AdapterError> {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(AdapterOutput::json(json!({"ok":true})))
        }
    }

    #[test]
    fn approval_policy_is_read_only_only_for_observation() {
        for action in ["capture", "status", "cua_browser_state", "list_sessions"] {
            assert!(!action_requires_approval(action), "{action}");
        }
        for action in ["start", "navigate", "click", "type", "scroll", "close"] {
            assert!(action_requires_approval(action), "{action}");
        }
    }

    #[test]
    fn auto_mode_preserves_command_configuration() {
        assert_eq!(AdapterMode::parse("auto").unwrap(), AdapterMode::Auto);
        assert_eq!(
            AdapterMode::parse("native-browser").unwrap(),
            AdapterMode::NativeBrowser
        );
        assert!(AdapterMode::parse("iframe").is_err());
    }

    #[test]
    fn renderer_emits_json_and_image_content() {
        let value = json!({
            "ok": true,
            "screenshot": {
                "attachmentId": "sha256:test",
                "mediaType": "image/png",
                "bytes": 8,
                "width": 1,
                "height": 1,
                "name": "browser.png"
            }
        });
        let blocks = render_output(&value).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], dsh_llm::ContentBlock::Text { .. }));
        assert!(matches!(blocks[1], dsh_llm::ContentBlock::Image { .. }));
    }

    #[test]
    fn output_values_must_stay_objects() {
        let mut value = Value::Array(vec![]);
        assert!(value.as_object_mut().is_none());
        let mut object = Value::Object(serde_json::Map::new());
        assert!(object.as_object_mut().is_some());
    }

    #[tokio::test]
    async fn registered_tool_exposes_schema_and_image_result_to_the_model() {
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .expect("system prompt");
        let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
        let store: Arc<dyn AttachmentStore> = Arc::new(TestAttachmentStore {
            limits: ImageAttachmentLimits {
                max_image_bytes: 10_000_000,
                max_images_per_message: 8,
                max_message_image_bytes: 20_000_000,
                max_image_pixels: 10_000_000,
                media_types: vec![ImageMediaType::Png],
            },
        });
        ctx.register_service(store);
        install_adapter(&ctx, 5_000, Arc::new(TestAdapter)).expect("computer-use tool");

        let schema = tools
            .schemas(None)
            .into_iter()
            .find(|schema| schema.name == "computer_use")
            .expect("model-facing tool declaration");
        assert!(schema.description.contains("isolated browser"));
        assert_eq!(schema.parameters["required"], json!(["action"]));
        assert!(schema.parameters["properties"].get("sessionId").is_some());

        let result = tools
            .execute(ToolExecutionInput {
                call_id: dsh_llm::call_id("computer-use-test"),
                root_call_id: None,
                name: "computer_use".to_string(),
                arguments: json!({"action":"capture","sessionId":"fixture"}),
                agent: None,
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(!result.is_error, "{:?}", result.error);
        assert_eq!(result.value.as_ref().unwrap()["adapter"], "test-browser");
        assert_eq!(result.content.len(), 2);
        assert!(matches!(
            result.content[0],
            dsh_llm::ContentBlock::Text { .. }
        ));
        assert!(matches!(
            result.content[1],
            dsh_llm::ContentBlock::Image { .. }
        ));
    }

    #[tokio::test]
    async fn shared_runtime_enforces_timeout_and_cancellation_outside_tool_pipeline() {
        let runtime = ComputerUseRuntime {
            ctx: Context::root(),
            adapter: Arc::new(SlowAdapter),
            timeout: Duration::from_millis(25),
            owner_agents: SyncMutex::new(HashMap::new()),
        };
        let timed_out = runtime
            .execute("owner", &json!({"action":"capture"}), Arc::new(|| false))
            .await
            .unwrap_err();
        assert_eq!(timed_out.code, "COMPUTER_USE_TIMEOUT");

        let cancelled = runtime
            .execute("owner", &json!({"action":"capture"}), Arc::new(|| true))
            .await
            .unwrap_err();
        assert_eq!(cancelled.code, "COMPUTER_USE_ABORTED");
    }

    #[tokio::test]
    async fn unavailable_native_browser_still_registers_the_runtime_and_tool() {
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
            .expect("system prompt");
        let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
        let root = std::env::temp_dir().join(format!(
            "dsh-computer-use-host-start-{}",
            uuid::Uuid::new_v4()
        ));
        let runtime = install(
            &ctx,
            Config {
                adapter: AdapterMode::NativeBrowser,
                command: String::new(),
                timeout_ms: 5_000,
                browser_executable: Some(root.join("missing-browser.exe")),
                browser_data_root: root.join("profiles"),
                browser_headless: true,
                max_browser_sessions: 1,
            },
        )
        .expect("optional browser discovery must be lazy");
        let error = runtime.availability().unwrap_err();
        assert_eq!(error.code, "COMPUTER_USE_BROWSER_NOT_FOUND");
        assert!(
            tools
                .schemas(None)
                .iter()
                .any(|schema| schema.name == "computer_use")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
