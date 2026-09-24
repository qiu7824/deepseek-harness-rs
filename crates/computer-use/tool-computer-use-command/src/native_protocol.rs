//! Native computer batches reuse the controller's ordinary authorization path.
use super::*;
use dsh_llm::computer_protocol::{TOOL_NAME, parse_call};
#[cfg(test)]
#[path = "native_protocol_tests.rs"]
mod tests;

#[derive(Clone)]
pub(super) struct Frame {
    pub identity: ComputerTargetIdentity,
    pub epoch: Arc<AtomicU64>,
    pub revision: u64,
    pub control_generation: u64,
    width: u64,
    height: u64,
    pub viewport: Value,
}
#[derive(Default)]
struct State {
    started: bool,
    frame: Option<Frame>,
}
struct ProtocolStatus(Value);
impl cordis::Service for ProtocolStatus {
    fn service_name(&self) -> &'static str {
        "nativeComputerProtocolStatus"
    }
}
pub(super) fn status(ctx: &Context) -> Value {
    ctx.get_typed::<Arc<ProtocolStatus>>("nativeComputerProtocolStatus", false)
        .map(|s| s.0.clone())
        .unwrap_or_else(|| json!({"enabled":false,"ready":false}))
}
pub fn native_protocol_failure(ctx: &Context, message: &str) {
    ctx.register_service(Arc::new(ProtocolStatus(
        json!({"enabled":true,"ready":false,"error":message}),
    )));
}
struct NativeProtocol {
    runtime: Arc<ComputerUseRuntime>,
    target: String,
    owners:
        SyncMutex<HashMap<String, (Weak<dyn dsh_agent::Agent>, Arc<tokio::sync::Mutex<State>>)>>,
}
fn error(code: &str, message: &str) -> AdapterError {
    AdapterError::new(code, message)
}

fn action_arguments(action: &Value) -> Result<Value, AdapterError> {
    let mut value = action.clone();
    let object = value.as_object_mut().unwrap();
    let kind = object.remove("type").unwrap().as_str().unwrap().to_string();
    object.insert(
        "action".into(),
        json!(match kind.as_str() {
            "screenshot" => "capture",
            "keypress" => "key",
            kind => kind,
        }),
    );
    if object.get("button").is_some_and(|button| button == "wheel") {
        object.insert("button".into(), json!("middle"));
    }
    if kind == "scroll" {
        for (from, to) in [("scroll_x", "deltaX"), ("scroll_y", "deltaY")] {
            let v = object.remove(from).unwrap();
            object.insert(to.into(), v);
        }
    }
    if let Some(keys) = object.get_mut("keys").and_then(Value::as_array_mut) {
        for key in keys {
            let mapped = match key.as_str().unwrap().to_ascii_uppercase().as_str() {
                "CTRL" => "Control",
                "CMD" | "COMMAND" | "META" => "Meta",
                "OPTION" => "Alt",
                "ESC" => "Escape",
                "DEL" => "Delete",
                _ => continue,
            };
            *key = json!(mapped);
        }
    }
    if kind == "type" && value["text"].as_str().unwrap().len() > 32768 {
        return Err(error(
            "COMPUTER_USE_INVALID_ARGUMENT",
            "Native text exceeds the controller limit",
        ));
    }
    if matches!(
        kind.as_str(),
        "click" | "double_click" | "move" | "drag" | "scroll"
    ) {
        super::browser::validate_pointer_modifiers(&value)?;
    }
    if kind == "keypress" {
        super::browser::validate_key_event(&value)?;
    }
    value["includeScreenshot"] = json!(false);
    value["waitMs"] = json!(0);
    Ok(value)
}
fn coordinates_fit(actions: &[Value], frame: &Frame) -> Result<(), AdapterError> {
    for action in actions {
        let points = if action["action"] == "drag" {
            action["path"]
                .as_array()
                .unwrap()
                .iter()
                .collect::<Vec<_>>()
        } else if action.get("x").is_some() {
            vec![action]
        } else {
            vec![]
        };
        for p in points {
            if p["x"].as_u64().is_none_or(|x| x >= frame.width)
                || p["y"].as_u64().is_none_or(|y| y >= frame.height)
            {
                return Err(error(
                    "COMPUTER_USE_FRAME_COORDINATE",
                    "Native coordinates are outside the observed frame",
                ));
            }
        }
    }
    Ok(())
}
impl NativeProtocol {
    fn arguments(&self, mut value: Value) -> Value {
        value["target"] = json!(self.target);
        value["sessionId"] = json!("native-protocol");
        value
    }
    async fn invoke(
        &self,
        owner: Arc<dyn dsh_agent::Agent>,
        value: &Value,
        signal: AbortPredicate,
        frame: Option<&Frame>,
    ) -> Result<AdapterOutput, AdapterError> {
        self.runtime
            .owner_agents
            .lock()
            .insert(owner.id().to_string(), Arc::downgrade(&owner));
        self.runtime
            .execute_inner(
                owner.id().to_string(),
                Some(owner),
                value,
                signal,
                ControlOrigin::Agent,
                frame,
            )
            .await
    }
    async fn capture(
        &self,
        owner: Arc<dyn dsh_agent::Agent>,
        state: &mut State,
        signal: AbortPredicate,
        recover: bool,
    ) -> Result<AdapterOutput, AdapterError> {
        let args = self.arguments(
            json!({"action":if state.started{"capture"}else{"start"},"includeScreenshot":true}),
        );
        // A new observation may recover an invalid old frame, but retains all
        // normal permission and human-control checks.
        let output = match self
            .invoke(
                owner.clone(),
                &args,
                signal.clone(),
                if recover { None } else { state.frame.as_ref() },
            )
            .await
        {
            Ok(output) => output,
            Err(error) => {
                state.frame = None;
                if matches!(
                    error.code.as_str(),
                    "COMPUTER_USE_SESSION_NOT_FOUND"
                        | "COMPUTER_USE_DESKTOP_DISCONNECTED"
                        | "COMPUTER_USE_BROWSER_EXITED"
                ) {
                    state.started = false;
                }
                return Err(error);
            }
        };
        state.started = true;
        if output.screenshot.is_none() {
            return Err(error(
                "COMPUTER_USE_FRAME_PENDING",
                "Controller returned no usable screenshot",
            ));
        }
        let identity = self
            .runtime
            .adapter
            .permission_identity(
                &AdapterRequest::from_arguments(&args)?.with_owner_id(owner.id().to_string()),
                signal,
            )
            .await?;
        if output.value["targetRevision"].as_str() != Some(identity.target_revision.as_str()) {
            return Err(error(
                "COMPUTER_USE_APP_IDENTITY_CHANGED",
                "Target changed while capturing a frame",
            ));
        }
        let width = output
            .value
            .pointer("/state/viewport/width")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                error(
                    "COMPUTER_USE_FRAME_PENDING",
                    "Observed frame width is missing",
                )
            })?;
        let height = output
            .value
            .pointer("/state/viewport/height")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                error(
                    "COMPUTER_USE_FRAME_PENDING",
                    "Observed frame height is missing",
                )
            })?;
        let epoch = self
            .runtime
            .observation_epochs
            .lock()
            .get(owner.id().as_str())
            .cloned()
            .ok_or_else(|| error("COMPUTER_USE_FRAME_STALE", "Observation expired"))?;
        let revision = output.value["observationRevision"]
            .as_u64()
            .ok_or_else(|| {
                error(
                    "COMPUTER_USE_FRAME_STALE",
                    "Observation revision is missing",
                )
            })?;
        if epoch.load(Ordering::SeqCst) != revision {
            return Err(error(
                "COMPUTER_USE_FRAME_STALE",
                "State changed while capturing a frame",
            ));
        }
        let control_generation = output
            .value
            .pointer("/control/generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                error(
                    "COMPUTER_USE_STALE_CONTROL",
                    "Control generation is missing",
                )
            })?;
        state.frame = Some(Frame {
            identity,
            epoch,
            revision,
            control_generation,
            width,
            height,
            viewport: output.value["state"]["viewport"].clone(),
        });
        Ok(output)
    }
    async fn execute(
        &self,
        owner: Arc<dyn dsh_agent::Agent>,
        call_id: &str,
        args: &Value,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        if !args.as_object().is_some_and(|args| {
            args.keys()
                .all(|k| matches!(k.as_str(), "actions" | "pendingSafetyChecks"))
        }) {
            return Err(error(
                "COMPUTER_USE_INVALID_ARGUMENT",
                "Unknown native computer arguments",
            ));
        }
        let (_,parsed)=parse_call(&json!({"type":"computer_call","call_id":call_id,"actions":args["actions"],"pending_safety_checks":args.get("pendingSafetyChecks").cloned().unwrap_or_else(||json!([]))})).map_err(|e|AdapterError::new("COMPUTER_USE_INVALID_ARGUMENT",e))?;
        if !parsed["pendingSafetyChecks"].as_array().unwrap().is_empty() {
            return Err(error(
                "COMPUTER_USE_HUMAN_REQUIRED",
                "Provider safety checks require explicit human acknowledgement",
            ));
        }
        let actions = parsed["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(action_arguments)
            .collect::<Result<Vec<_>, _>>()?;
        let adapter = self
            .runtime
            .adapter_id_for(&self.arguments(json!({"action":"capture"})))?;
        for action in &actions {
            super::arguments::normalize(adapter, action)?;
        }
        let state = {
            let mut owners = self.owners.lock();
            owners.retain(|_, (owner, _)| owner.strong_count() > 0);
            if owners.len() >= 64 && !owners.contains_key(owner.id().as_str()) {
                return Err(error(
                    "COMPUTER_USE_SESSION_LIMIT",
                    "Native computer owner limit reached",
                ));
            }
            owners
                .entry(owner.id().to_string())
                .or_insert_with(|| {
                    (
                        Arc::downgrade(&owner),
                        Arc::new(tokio::sync::Mutex::new(State::default())),
                    )
                })
                .1
                .clone()
        };
        let mut state = tokio::select! {state=state.lock()=>state,_=wait_for_cancel(signal.clone())=>return Err(AdapterError::cancelled())};
        if signal() {
            return Err(AdapterError::cancelled());
        }
        let mut frame_output = None;
        if actions[0]["action"] == "capture" {
            state.frame = None;
            frame_output = Some(
                self.capture(owner.clone(), &mut state, signal.clone(), true)
                    .await?,
            );
        }
        let frame = state.frame.as_ref().ok_or_else(|| {
            error(
                "COMPUTER_USE_FRAME_REQUIRED",
                "Start native computer interaction with a screenshot action",
            )
        })?;
        coordinates_fit(&actions, frame)?;
        let result=async {
            for (i,action) in actions.iter().enumerate() {
                if signal(){return Err(AdapterError::cancelled());}
                if action["action"]=="capture" {
                    if i!=0 {frame_output=Some(self.capture(owner.clone(),&mut state,signal.clone(),false).await?);}
                    continue;
                }
                if action["action"]=="wait" {tokio::select!{_=tokio::time::sleep(Duration::from_secs(1))=>{},_=wait_for_cancel(signal.clone())=>return Err(AdapterError::cancelled())};continue;}
                coordinates_fit(std::slice::from_ref(action),state.frame.as_ref().unwrap())?;
                let output=self.invoke(owner.clone(),&self.arguments(action.clone()),signal.clone(),state.frame.as_ref()).await?;
                state.frame.as_mut().unwrap().revision=output.value["observationRevision"].as_u64().ok_or_else(||error("COMPUTER_USE_FRAME_STALE","Observation revision is missing"))?;
                frame_output=None;
            }
            if frame_output.is_none(){frame_output=Some(self.capture(owner.clone(),&mut state,signal.clone(),false).await?);}
            Ok(frame_output.take().unwrap())
        }.await;
        if result.is_err() {
            state.frame = None;
        }
        result
    }
}

pub fn install_native_protocol(ctx: &Context, target: &str) -> Result<(), String> {
    if !matches!(target, "local" | "browser") {
        return Err("Native computer target must be local or browser".into());
    }
    if ctx
        .get_typed::<Arc<dyn ComputerPermissionService>>("computerPermissions", false)
        .is_none()
    {
        return Err("Native computer requires application permissions".into());
    }
    let runtime = ctx
        .get_typed::<Arc<ComputerUseRuntime>>("computerUse", false)
        .ok_or("Computer controller is unavailable")?
        .as_ref()
        .clone();
    let selected = runtime
        .adapter_id_for(&json!({"target":target}))
        .map_err(|e| e.message)?;
    if !matches!(
        (target, selected),
        ("local", "native-desktop") | ("browser", "native-browser")
    ) {
        return Err("Selected adapter does not support native computer actions".into());
    }
    let attachments = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .ok_or("Native computer requires attachment storage")?
        .as_ref()
        .clone();
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("Tool runtime is unavailable")?
        .as_ref()
        .clone();
    let protocol = Arc::new(NativeProtocol {
        runtime,
        target: target.into(),
        owners: Default::default(),
    });
    tools.register(ctx,ToolDefinition{
        name:TOOL_NAME.into(),description:"Operate the configured computer with native ordered actions. Begin with a screenshot; coordinates refer to that observed frame. Application authorization, manual control, cancellation and target identity remain enforced.".into(),
        parameters:json!({"type":"object","title":"dsh-native-computer-v1","properties":{"actions":{"type":"array","minItems":1,"maxItems":64,"items":{"type":"object"}},"pendingSafetyChecks":{"type":"array","items":{"type":"object"}}},"required":["actions"],"additionalProperties":false}),
        output:ToolOutputDefinition{schema:json!({}),render:Arc::new(|_,value|render_output(value)),presentation_meta:None},
        timeout_ms:Some(120_000),is_concurrency_safe:Some(Arc::new(|_|false)),
        execute:Arc::new(move|args,run|{let protocol=protocol.clone();let attachments=attachments.clone();let owner=run.agent.clone();let signal=run.signal.lock().clone();let id=run.call_id.to_string();let args=args.clone();Box::pin(async move {
            let owner=owner.ok_or_else(||tool_body_error(error("COMPUTER_USE_OWNER_REQUIRED","Native computer needs an owning agent")))?;
            let mut output=protocol.execute(owner.clone(),&id,&args,signal.clone()).await.map_err(tool_body_error)?;
            if signal(){return Err(tool_body_error(AdapterError::cancelled()));}
            let screenshot=output.screenshot.take().ok_or_else(||tool_body_error(error("COMPUTER_USE_FRAME_PENDING","Native result has no screenshot")))?;
            let saved=attachments.save_image(&SaveImageAttachment{data:screenshot.data,media_type:image_media_type(&screenshot.media_type).map_err(tool_body_error)?,name:screenshot.name}).await.map_err(|e|ToolBodyError::coded(e.to_string(),"ComputerUseError","COMPUTER_USE_SCREENSHOT_REJECTED"))?;
            output.value["screenshot"]=serde_json::to_value(saved).map_err(|e|ToolBodyError::plain(e.to_string()))?;
            publish_control_activity(owner.session(),&protocol.arguments(json!({"action":"capture"})),&output.value,&id);
            Ok(output.value)
        })}),finalize_content:None,present_call:None,present_result:None,
    })?;
    ctx.register_service(Arc::new(ProtocolStatus(
        json!({"enabled":true,"ready":true,"target":target}),
    )));
    Ok(())
}
