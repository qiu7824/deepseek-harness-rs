//! Persistent JavaScript bridge. Every action re-enters ToolRuntime using the
//! current root execution token; kernels cannot authorize their own actions.
use cordis::Context;
use dsh_subprocess::*;
use dsh_tools::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const SOURCE: &str = include_str!("../assets/computer-js.cjs");
const PARSER: &str = include_str!("../assets/acorn.cjs");
struct Kernel {
    child: Arc<dyn SubprocessHandle>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    images: HashMap<String, Value>,
}
impl Drop for Kernel {
    fn drop(&mut self) {
        self.child.terminate();
    }
}
struct Slot {
    kernel: tokio::sync::Mutex<Option<Kernel>>,
    generation: AtomicU64,
    last_used: parking_lot::Mutex<Instant>,
}
struct ComputerJs {
    node: String,
    root: PathBuf,
    processes: Arc<dyn SubprocessRuntime>,
    tools: Weak<ToolRuntime>,
    slots: parking_lot::Mutex<HashMap<String, Arc<Slot>>>,
}
impl cordis::Service for ComputerJs {
    fn service_name(&self) -> &'static str {
        "computerUseJs"
    }
}
fn failure(code: &str, completed: u64, uncertain: bool) -> ToolBodyError {
    ToolBodyError::coded(json!({"code":code,"completedActions":completed,"uncertainAction":uncertain,"kernelReset":true}).to_string(),"ComputerUseJsError",code)
}
impl ComputerJs {
    async fn run(&self, code: &str, execution: Arc<ToolExecution>) -> Result<Value, ToolBodyError> {
        let value = self.evaluate(code, execution).await?;
        if value["ok"] != true {
            return Err(ToolBodyError::coded(
                value.to_string(),
                "ComputerUseJsError",
                "COMPUTER_USE_JS_FAILED",
            ));
        }
        Ok(value)
    }
    async fn spawn(&self) -> Result<Kernel, ToolBodyError> {
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("{SOURCE}{PARSER}").as_bytes())
        );
        let root = self.root.join(digest);
        for (name, source) in [("computer-js.cjs", SOURCE), ("acorn.cjs", PARSER)] {
            let path = root.join(name);
            if tokio::fs::read(&path).await.ok().as_deref() != Some(source.as_bytes()) {
                dsh_atomic_write::write_file_atomic(
                    &path,
                    source.as_bytes(),
                    dsh_atomic_write::WriteFileAtomicOptions {
                        mode: 0o600,
                        dir_mode: Some(0o700),
                    },
                )
                .await
                .map_err(|e| ToolBodyError::plain(e.to_string()))?;
            }
        }
        let argv = vec![
            self.node.clone(),
            "--permission".into(),
            "--allow-worker".into(),
            "--max-old-space-size=192".into(),
            format!("--allow-fs-read={}", root.display()),
            root.join("computer-js.cjs").to_string_lossy().into_owned(),
        ];
        let child = self
            .processes
            .spawn(SubprocessSpawnSpec {
                argv,
                cwd: root.to_string_lossy().into_owned(),
                stdio: SubprocessStdio {
                    stdin: SubprocessStdinMode::Pipe,
                    stdout: SubprocessOutputMode::Pipe,
                    stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                        max_bytes: 8192,
                        spill: None,
                    }),
                },
                grace_ms: 1000,
                signal: None,
                env: Some(vec![("NODE_OPTIONS".into(), None)]),
            })
            .map_err(ToolBodyError::plain)?;
        Ok(Kernel {
            input: child
                .stdin()
                .ok_or_else(|| ToolBodyError::plain("Kernel stdin unavailable"))?,
            output: BufReader::new(
                child
                    .stdout()
                    .ok_or_else(|| ToolBodyError::plain("Kernel stdout unavailable"))?,
            ),
            child,
            images: HashMap::new(),
        })
    }
    fn slot(&self, owner: &str) -> Result<Arc<Slot>, ToolBodyError> {
        let mut slots = self.slots.lock();
        if let Some(slot) = slots.get(owner) {
            return Ok(slot.clone());
        }
        if slots.len() >= 4 {
            return Err(failure("COMPUTER_USE_JS_SESSION_LIMIT", 0, false));
        }
        let slot = Arc::new(Slot {
            kernel: tokio::sync::Mutex::new(None),
            generation: AtomicU64::new(0),
            last_used: parking_lot::Mutex::new(Instant::now()),
        });
        slots.insert(owner.into(), slot.clone());
        Ok(slot)
    }
    async fn reset(&self, owner: &str) {
        let slot = self.slots.lock().remove(owner);
        if let Some(slot) = slot {
            slot.generation.fetch_add(1, Ordering::SeqCst);
            let kernel = slot.kernel.lock().await.take();
            if let Some(kernel) = kernel {
                kernel.child.terminate();
                let _ = kernel.child.wait_for_exit(None).await;
            }
        }
    }

    async fn reap_idle(&self) {
        let slots = self
            .slots
            .lock()
            .iter()
            .map(|(owner, slot)| (owner.clone(), slot.clone()))
            .collect::<Vec<_>>();
        for (owner, slot) in slots {
            let Ok(mut guard) = slot.kernel.try_lock() else {
                continue;
            };
            if slot.last_used.lock().elapsed() < Duration::from_secs(300) {
                continue;
            }
            slot.generation.fetch_add(1, Ordering::SeqCst);
            let kernel = guard.take();
            {
                let mut slots = self.slots.lock();
                if slots
                    .get(&owner)
                    .is_some_and(|entry| Arc::ptr_eq(entry, &slot))
                {
                    slots.remove(&owner);
                }
            }
            drop(guard);
            if let Some(kernel) = kernel {
                kernel.child.terminate();
                let _ = kernel.child.wait_for_exit(None).await;
            }
        }
    }
    async fn evaluate(
        &self,
        code: &str,
        execution: Arc<ToolExecution>,
    ) -> Result<Value, ToolBodyError> {
        let owner = execution
            .agent
            .as_ref()
            .ok_or_else(|| ToolBodyError::plain("Computer Use JS requires a session"))?
            .id()
            .as_str()
            .to_string();
        let slot = self.slot(&owner)?;
        let generation = slot.generation.load(Ordering::SeqCst);
        let signal = execution.signal.lock().clone();
        let mut guard = tokio::select! {guard=slot.kernel.lock()=>guard,_=crate::wait_for_cancel(signal.clone())=>return Err(failure("COMPUTER_USE_ABORTED",0,false))};
        if signal() || slot.generation.load(Ordering::SeqCst) != generation {
            return Err(failure("COMPUTER_USE_ABORTED", 0, false));
        }
        if guard.is_none() {
            *guard = Some(self.spawn().await?);
        }
        *slot.last_used.lock() = Instant::now();
        let active = Arc::new(AtomicBool::new(true));
        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _lease = Lease(active.clone());
        let eval_id = uuid::Uuid::new_v4().to_string();
        let mut completed = 0u64;
        let mut attempted = 0u64;
        let mut uncertain = false;
        let mut observations = Vec::new();
        let cancelled = {
            let active = active.clone();
            let slot = slot.clone();
            let signal = signal.clone();
            Arc::new(move || {
                !active.load(Ordering::SeqCst)
                    || signal()
                    || slot.generation.load(Ordering::SeqCst) != generation
            }) as crate::AbortPredicate
        };
        let work = async {
            let kernel = guard.as_mut().unwrap();
            kernel
                .input
                .write_all(
                    format!(
                        "{}\n",
                        json!({"type":"eval","evalId":eval_id,"code":code,"timeoutMs":60000})
                    )
                    .as_bytes(),
                )
                .await
                .map_err(|_| failure("COMPUTER_USE_KERNEL_PIPE", completed, uncertain))?;
            kernel
                .input
                .flush()
                .await
                .map_err(|_| failure("COMPUTER_USE_KERNEL_PIPE", completed, uncertain))?;
            loop {
                let mut bytes = Vec::new();
                let read = (&mut kernel.output)
                    .take(2 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut bytes)
                    .await
                    .map_err(|_| failure("COMPUTER_USE_KERNEL_PIPE", completed, uncertain))?;
                if read == 0 {
                    return Err(ToolBodyError::coded(
                        "Computer Use JS kernel exited. A packaged Node 25+ runtime is required; check the runtime diagnostics.",
                        "ComputerUseJsError",
                        "COMPUTER_USE_KERNEL_EXITED",
                    ));
                }
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err(failure("COMPUTER_USE_OUTPUT_LIMIT", completed, uncertain));
                }
                let frame: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| failure("COMPUTER_USE_KERNEL_PROTOCOL", completed, uncertain))?;
                if frame["evalId"] != eval_id {
                    return Err(failure(
                        "COMPUTER_USE_STALE_EVALUATION",
                        completed,
                        uncertain,
                    ));
                }
                if frame["type"] == "action" {
                    attempted += 1;
                    if attempted > 64 {
                        return Err(failure("COMPUTER_USE_ACTION_LIMIT", completed, uncertain));
                    }
                    let tools = self
                        .tools
                        .upgrade()
                        .ok_or_else(|| ToolBodyError::plain("Tool runtime unavailable"))?;
                    uncertain = true;
                    let result = tools
                        .execute(ToolExecutionInput {
                            call_id: dsh_llm::call_id(format!(
                                "{}:computer-js:{attempted}",
                                execution.call_id
                            )),
                            root_call_id: Some(execution.root_call_id.clone()),
                            name: "computer_use".into(),
                            arguments: frame["arguments"].clone(),
                            agent: execution.agent.clone(),
                            parent: Some(execution.token),
                            signal: cancelled.clone(),
                        })
                        .await;
                    uncertain = false;
                    if let Some(error) = result.error.as_ref() {
                        if let Some(info) = &error.info {
                            if matches!(
                                info.code.as_str(),
                                "USER_APPROVAL_DENIED"
                                    | "USER_APPROVAL_CANCELLED"
                                    | "USER_APPROVAL_TIMED_OUT"
                                    | "USER_APPROVAL_UNAVAILABLE"
                                    | "COMPUTER_USE_MANUAL_CONTROL"
                            ) {
                                return Err(ToolBodyError::coded(
                                    format!("{}; completedActions={completed}", error.message),
                                    &info.name,
                                    &info.code,
                                ));
                            }
                        }
                    }
                    if !result.is_error {
                        completed += 1;
                        if let Some(value) = &result.value {
                            if let Some(image) = value.get("screenshot") {
                                if let Some(id) = image["attachmentId"].as_str() {
                                    if kernel.images.len() >= 256 {
                                        kernel.images.clear();
                                    }
                                    kernel.images.insert(id.into(), image.clone());
                                }
                            }
                            observations.push(value.clone());
                            if observations.len() > 8 {
                                observations.remove(0);
                            }
                        }
                    }
                    let response = json!({"type":"action_result","evalId":eval_id,"id":frame["id"],"ok":!result.is_error,"value":result.value,"error":result.error.as_ref().map(|error|error.message.clone())});
                    kernel
                        .input
                        .write_all(format!("{response}\n").as_bytes())
                        .await
                        .map_err(|_| failure("COMPUTER_USE_KERNEL_PIPE", completed, false))?;
                    kernel
                        .input
                        .flush()
                        .await
                        .map_err(|_| failure("COMPUTER_USE_KERNEL_PIPE", completed, false))?;
                } else if frame["type"] == "result" {
                    let mut images = Vec::new();
                    for observation in &observations {
                        if let Some(image) = observation.get("screenshot") {
                            images.push(image.clone());
                        }
                    }
                    for log in frame["logs"].as_array().into_iter().flatten() {
                        if log["type"] == "image" {
                            let image = log["image"].get("screenshot").unwrap_or(&log["image"]);
                            let id = image["attachmentId"].as_str().ok_or_else(|| {
                                failure("COMPUTER_USE_IMAGE_REFERENCE", completed, false)
                            })?;
                            images.push(kernel.images.get(id).cloned().ok_or_else(|| {
                                failure("COMPUTER_USE_IMAGE_REFERENCE", completed, false)
                            })?);
                        }
                    }
                    images.dedup_by(|a, b| a["attachmentId"] == b["attachmentId"]);
                    if images.len() > 4 {
                        images = images.split_off(images.len() - 4);
                    }
                    return Ok(
                        json!({"ok":frame["ok"],"value":frame["value"],"error":frame["error"],"logs":frame["logs"],"completedActions":completed,"kernelReset":frame["reset"],"observations":observations,"images":images}),
                    );
                } else {
                    return Err(failure(
                        "COMPUTER_USE_KERNEL_PROTOCOL",
                        completed,
                        uncertain,
                    ));
                }
            }
        };
        let outcome = tokio::select! {
            result=tokio::time::timeout(Duration::from_secs(65),work)=>result.map_err(|_|failure("COMPUTER_USE_JS_TIMEOUT",completed,uncertain)).and_then(|result|result),
            _=crate::wait_for_cancel(cancelled.clone())=>Err(failure("COMPUTER_USE_ABORTED",completed,uncertain)),
        };
        active.store(false, Ordering::SeqCst);
        *slot.last_used.lock() = Instant::now();
        if outcome.is_err() {
            if let Some(kernel) = guard.take() {
                kernel.child.terminate();
                let _ = kernel.child.wait_for_exit(None).await;
            }
        }
        outcome
    }
}

pub fn install_js(ctx: &Context, node: String, root: PathBuf) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("tools unavailable")?
        .as_ref()
        .clone();
    let processes = ctx
        .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
        .ok_or("subprocess unavailable")?
        .as_ref()
        .clone();
    let service = Arc::new(ComputerJs {
        node,
        root,
        processes,
        tools: Arc::downgrade(&tools),
        slots: Default::default(),
    });
    ctx.register_service(service.clone());
    for event in ["session/disposed", "workspace/session-deleted"] {
        let weak = Arc::downgrade(&service);
        futures::executor::block_on(ctx.on(
            event,
            Arc::new(move |_, args| {
                let weak = weak.clone();
                let owner = args.first().and_then(|value| {
                    cordis::downcast::<dsh_session::Session>(value)
                        .map(|session| session.id().as_str().to_string())
                        .or_else(|| {
                            cordis::downcast::<dsh_session::SessionId>(value)
                                .map(|id| id.as_str().to_string())
                        })
                });
                Box::pin(async move {
                    if let (Some(service), Some(owner)) = (weak.upgrade(), owner) {
                        service.reset(&owner).await;
                    }
                    None
                })
            }),
            cordis::EventOptions::default().global(true),
        ));
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_reaper = stopped.clone();
    let weak_reaper = Arc::downgrade(&service);
    tokio::spawn(async move {
        while !stop_reaper.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(service) = weak_reaper.upgrade() else {
                break;
            };
            service.reap_idle().await;
        }
    });
    for reset in [false, true] {
        let service = service.clone();
        tools.register(ctx,ToolDefinition{name:if reset{"computer_use_js_reset"}else{"computer_use_js"}.into(),description:if reset{"Reset this session's Computer Use JavaScript kernel and invalidate its variables."}else{"Run persistent JavaScript for Computer Use. Variables persist across calls in this conversation. Use let app = await cua.getApp(name or windowRef), await app.getAXStateAndScreenshot(), app.click(elementId or [x,y]), app.setValue(elementId,text), app.typeText(text), app.pressKey('Control+a'), app.scroll(...). Use cua.getState() to discover windows. Default target is the Host computer; cua.remote().perform(...) uses only the bound UU device; cua.browser().perform(...) uses the isolated browser. nodeRepl.write(value) and nodeRepl.emitImage(observation) produce output. No modules, files, network or processes are accessible to code. Observe before acting; stale element IDs are rejected. Await all actions. A timeout resets variables. Every action receives normal Host authorization."}.into(),parameters:if reset{json!({"type":"object","properties":{},"additionalProperties":false})}else{json!({"type":"object","properties":{"code":{"type":"string","minLength":1,"maxLength":65536}},"required":["code"],"additionalProperties":false})},output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|{let mut blocks=vec![dsh_llm::ContentBlock::Text{text:value.to_string()}];for image in value["images"].as_array().into_iter().flatten(){blocks.push(dsh_llm::ContentBlock::Image{attachment:serde_json::from_value(image.clone()).map_err(|e|e.to_string())?});}Ok(blocks)}),presentation_meta:None},timeout_ms:Some(70000),is_concurrency_safe:Some(Arc::new(move |_|reset)),execute:Arc::new(move|args,run|{let service=service.clone();let code=args["code"].as_str().unwrap_or("").to_string();let execution=run.execution.clone();Box::pin(async move{if reset{let id=execution.agent.as_ref().ok_or_else(||ToolBodyError::plain("Session required"))?.id().as_str().to_string();service.reset(&id).await;Ok(json!({"reset":true}))}else{service.run(&code,execution).await}})}),finalize_content:None,present_call:None,present_result:None})?;
    }
    let weak = Arc::downgrade(&service);
    let _ = ctx.effect(
        "computerUseJs.shutdown",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                stopped.store(true, Ordering::SeqCst);
                let weak = weak.clone();
                Box::pin(async move {
                    if let Some(service) = weak.upgrade() {
                        let owners = service.slots.lock().keys().cloned().collect::<Vec<_>>();
                        for owner in owners {
                            service.reset(&owner).await;
                        }
                    }
                })
            }))
        }),
    );
    Ok(())
}
