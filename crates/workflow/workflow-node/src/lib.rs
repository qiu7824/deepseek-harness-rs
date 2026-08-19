//! Fresh-Node workflow engine over the repository's `CodeRuntime` and
//! `SubagentRuntime` seams.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cordis::Context;
use dsh_code_runtime::{
    CodeAbort, CodeBindingFunction, CodeBindingNamespace, CodeRunFailureKind, CodeRunRequest,
    CodeRuntime,
};
use dsh_llm::ContentBlock;
use dsh_subagent::{SubagentRuntime, SubagentStartRequest, SubagentStopReason};
use dsh_workflow::{
    WorkflowAgentEndInfo, WorkflowAgentInfo, WorkflowAgentOutcome, WorkflowEngine, WorkflowError,
    WorkflowMeta, WorkflowResult, WorkflowRun, WorkflowRunId, WorkflowStartRequest,
    validate_start_request, workflow_run_id,
};
use serde_json::Value;
use tokio::sync::Notify;

/// Deployment-level limits. Concurrency and total-call fields are present
/// from the first slice even though their enforcement follows in later TDD
/// cycles.
#[derive(Debug, Clone)]
pub struct Config {
    pub provider: String,
    pub max_concurrent_agents: usize,
    pub max_total_agents: u64,
    pub dispose_grace_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "spawn".to_string(),
            max_concurrent_agents: 0,
            max_total_agents: 1_000,
            dispose_grace_ms: 5_000,
        }
    }
}

/// Workflow implementation backed by the `codeRuntime` and `subagents`
/// services mounted on its context.
pub struct NodeWorkflowEngine {
    ctx: Context,
    code_runtime: Arc<dyn CodeRuntime>,
    subagents: Arc<SubagentRuntime>,
    config: Config,
    accepting: Arc<AtomicBool>,
    admission: Arc<parking_lot::Mutex<()>>,
    runs: Arc<parking_lot::Mutex<HashMap<String, Arc<NodeWorkflowRun>>>>,
}

impl NodeWorkflowEngine {
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, WorkflowError> {
        validate_config(&config)?;
        let code_runtime = ctx
            .get_typed::<Arc<dyn CodeRuntime>>("codeRuntime", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                WorkflowError::new(
                    "workflow-node requires codeRuntime",
                    dsh_workflow::WorkflowErrorCode::InvalidArgument,
                )
            })?;
        let subagents = ctx
            .get_typed::<Arc<SubagentRuntime>>("subagents", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                WorkflowError::new(
                    "workflow-node requires subagents",
                    dsh_workflow::WorkflowErrorCode::InvalidArgument,
                )
            })?;
        Ok(Arc::new(Self {
            ctx: ctx.clone(),
            code_runtime,
            subagents,
            config,
            accepting: Arc::new(AtomicBool::new(true)),
            admission: Arc::new(parking_lot::Mutex::new(())),
            runs: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }))
    }
}

impl WorkflowEngine for NodeWorkflowEngine {
    fn context(&self) -> &Context {
        &self.ctx
    }

    fn start(&self, request: WorkflowStartRequest) -> Result<Arc<dyn WorkflowRun>, WorkflowError> {
        let _admission = self.admission.lock();
        if !self.accepting.load(Ordering::Acquire) {
            return Err(WorkflowError::new(
                "workflow engine is disposed",
                dsh_workflow::WorkflowErrorCode::InvalidArgument,
            ));
        }
        validate_start_request(&request)?;
        let provider = request
            .subagent_provider
            .clone()
            .unwrap_or_else(|| self.config.provider.clone());
        if self.subagents.get_provider(&provider).is_none() {
            return Err(WorkflowError::new(
                format!("no subagent provider registered for \"{provider}\""),
                dsh_workflow::WorkflowErrorCode::AgentStart,
            ));
        }

        let id = workflow_run_id(uuid::Uuid::new_v4().to_string());
        let (run, start_task) = NodeWorkflowRun::spawn(
            self.ctx.clone(),
            self.code_runtime.clone(),
            self.subagents.clone(),
            id,
            request,
            provider,
            self.config.clone(),
            self.runs.clone(),
        );
        self.runs
            .lock()
            .insert(run.id().as_str().to_string(), run.clone());
        let _ = start_task.send(());
        Ok(run)
    }
}

impl NodeWorkflowEngine {
    pub fn dispose(&self) -> futures::future::BoxFuture<'static, ()> {
        let runs = {
            let _admission = self.admission.lock();
            self.accepting.store(false, Ordering::Release);
            self.runs.lock().values().cloned().collect::<Vec<_>>()
        };
        Box::pin(async move {
            for run in runs {
                run.dispose().await;
            }
        })
    }
}

struct NodeWorkflowRun {
    ctx: Context,
    id: WorkflowRunId,
    meta: WorkflowMeta,
    cancelled: Arc<AtomicBool>,
    cancel_reason: parking_lot::Mutex<Option<String>>,
    result: Arc<parking_lot::Mutex<Option<WorkflowResult>>>,
    settled: Arc<Notify>,
    task: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    active_children: Arc<parking_lot::Mutex<HashMap<u64, Arc<dyn dsh_subagent::SubagentRun>>>>,
    dispose_grace_ms: u64,
    runs: Arc<parking_lot::Mutex<HashMap<String, Arc<NodeWorkflowRun>>>>,
}

impl NodeWorkflowRun {
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        ctx: Context,
        code_runtime: Arc<dyn CodeRuntime>,
        subagents: Arc<SubagentRuntime>,
        id: WorkflowRunId,
        request: WorkflowStartRequest,
        provider: String,
        config: Config,
        runs: Arc<parking_lot::Mutex<HashMap<String, Arc<NodeWorkflowRun>>>>,
    ) -> (Arc<Self>, tokio::sync::oneshot::Sender<()>) {
        let run = Arc::new(Self {
            ctx: ctx.clone(),
            id,
            meta: request.meta.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_reason: parking_lot::Mutex::new(None),
            result: Arc::new(parking_lot::Mutex::new(None)),
            settled: Arc::new(Notify::new()),
            task: parking_lot::Mutex::new(None),
            active_children: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            dispose_grace_ms: config.dispose_grace_ms,
            runs: runs.clone(),
        });
        let owned = run.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            if start_rx.await.is_err() {
                return;
            }
            ctx.emit(
                "workflow/start",
                vec![
                    cordis::arc(owned.id.clone()),
                    cordis::arc(owned.meta.clone()),
                ],
            );
            let result = drive(
                &ctx,
                code_runtime,
                subagents,
                request,
                provider,
                owned.cancelled.clone(),
                owned.active_children.clone(),
                config,
            )
            .await;
            // Complete terminal publication in one non-cancellable sync block:
            // first terminal value wins, and its event/wakeup/registry removal
            // cannot be split by a concurrent task abort.
            let published = {
                let mut slot = owned.result.lock();
                if slot.is_none() {
                    *slot = Some(result.clone());
                    true
                } else {
                    false
                }
            };
            if published {
                ctx.emit(
                    "workflow/end",
                    vec![cordis::arc(owned.id.clone()), cordis::arc(result)],
                );
                owned.settled.notify_waiters();
            }
            runs.lock().remove(owned.id.as_str());
        });
        *run.task.lock() = Some(task);
        (run, start_tx)
    }
}

impl WorkflowRun for NodeWorkflowRun {
    fn id(&self) -> &WorkflowRunId {
        &self.id
    }

    fn meta(&self) -> &WorkflowMeta {
        &self.meta
    }

    fn result(&self) -> futures::future::BoxFuture<'static, WorkflowResult> {
        let result = self.result.clone();
        let settled = self.settled.clone();
        Box::pin(async move {
            loop {
                let notified = settled.notified();
                if let Some(result) = result.lock().clone() {
                    return result;
                }
                notified.await;
            }
        })
    }

    fn cancel(&self, reason: Option<String>) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            *self.cancel_reason.lock() =
                Some(reason.unwrap_or_else(|| "workflow cancelled".to_string()));
        }
    }

    fn dispose(&self) -> futures::future::BoxFuture<'static, ()> {
        self.cancel(Some("workflow disposed".to_string()));
        let result_state = self.result.clone();
        let settled = self.settled.clone();
        let cancelled_result = WorkflowResult::cancelled(self.cancel_reason.lock().clone(), 0);
        let ctx = self.ctx.clone();
        let id = self.id.clone();
        let runs = self.runs.clone();
        let task = self.task.lock().take();
        // Publish terminal cancellation and close task admission synchronously
        // with dispose(); callers may drop the returned future without
        // reopening a settlement or child-publication window.
        let published = {
            let mut slot = result_state.lock();
            if slot.is_none() {
                *slot = Some(cancelled_result.clone());
                true
            } else {
                false
            }
        };
        if published {
            ctx.emit(
                "workflow/end",
                vec![
                    cordis::arc(id.clone()),
                    cordis::arc(cancelled_result.info()),
                ],
            );
            settled.notify_waiters();
        }
        runs.lock().remove(id.as_str());
        if let Some(task) = &task {
            task.abort();
        }
        let active_children = self.active_children.clone();
        let grace = std::time::Duration::from_millis(self.dispose_grace_ms);
        Box::pin(async move {
            // The workflow task owns the binding futures. Abort it first so no
            // late child can be published while the registry is draining.
            if let Some(task) = task {
                task.abort();
                let _ = tokio::time::timeout(grace, task).await;
            }
            let children = active_children.lock().values().cloned().collect::<Vec<_>>();
            let cleanup = async move {
                futures::future::join_all(children.iter().map(|child| child.dispose())).await;
            };
            let _ = tokio::time::timeout(grace, cleanup).await;
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    ctx: &Context,
    code_runtime: Arc<dyn CodeRuntime>,
    subagents: Arc<SubagentRuntime>,
    request: WorkflowStartRequest,
    provider: String,
    cancelled: Arc<AtomicBool>,
    active_children: Arc<parking_lot::Mutex<HashMap<u64, Arc<dyn dsh_subagent::SubagentRun>>>>,
    config: Config,
) -> WorkflowResult {
    let parent = request.parent.clone();
    let agents_started = Arc::new(AtomicU64::new(0));
    let sequence = agents_started.clone();
    let event_ctx = ctx.clone();
    let child_abort = cancelled.clone();
    let external_abort = request.signal.clone();
    let total_cap = request
        .max_total_agents
        .unwrap_or(config.max_total_agents)
        .min(config.max_total_agents);
    let concurrency = (config.max_concurrent_agents > 0)
        .then(|| Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_agents)));
    let binding: CodeBindingFunction = Arc::new(move |input| {
        let subagents = subagents.clone();
        let parent = parent.clone();
        let provider = provider.clone();
        let event_ctx = event_ctx.clone();
        let sequence = sequence.clone();
        let child_abort = child_abort.clone();
        let external_abort = external_abort.clone();
        let active_children = active_children.clone();
        let concurrency = concurrency.clone();
        Box::pin(async move {
            if child_abort.load(Ordering::Acquire)
                || external_abort.as_ref().is_some_and(|signal| signal())
            {
                panic!("workflow cancelled");
            }
            let permit = match concurrency {
                Some(semaphore) => Some(
                    semaphore
                        .acquire_owned()
                        .await
                        .unwrap_or_else(|_| panic!("workflow concurrency gate closed")),
                ),
                None => None,
            };
            let call = read_agent_input(input);
            let seq = sequence.fetch_add(1, Ordering::AcqRel) + 1;
            if seq > total_cap {
                sequence.fetch_sub(1, Ordering::AcqRel);
                panic!("workflow agent cap exceeded ({total_cap})");
            }
            let child = subagents
                .start(
                    &provider,
                    SubagentStartRequest {
                        label: call.label.clone(),
                        prompt: vec![ContentBlock::Text {
                            text: call.prompt.clone(),
                        }],
                        parent,
                        signal: {
                            let child_abort = child_abort.clone();
                            Arc::new(move || child_abort.load(Ordering::Acquire))
                        },
                        agent_options: None,
                        output_schema: call.schema.clone(),
                        max_depth: None,
                        tool_filter: None,
                        persona: None,
                    },
                )
                .await
                .unwrap_or_else(|error| panic!("workflow agent start failed: {error}"));
            // Close the async admission window: disposal may begin while the
            // provider is constructing the child. Such a late publication is
            // retired here and never escapes workflow ownership.
            if child_abort.load(Ordering::Acquire)
                || external_abort.as_ref().is_some_and(|signal| signal())
            {
                let _ = child.dispose().await;
                panic!("workflow cancelled during child admission");
            }
            active_children.lock().insert(seq, child.clone());
            if child_abort.load(Ordering::Acquire)
                || external_abort.as_ref().is_some_and(|signal| signal())
            {
                active_children.lock().remove(&seq);
                let _ = child.dispose().await;
                panic!("workflow cancelled during child publication");
            }
            let info = WorkflowAgentInfo {
                seq,
                label: call.label.unwrap_or(call.prompt),
                phase: None,
                child_id: child.id().clone(),
            };
            event_ctx.emit("workflow/agent-start", vec![cordis::arc(info.clone())]);
            let settled = match child.result().await {
                Ok(settled) => settled,
                Err(error) => {
                    let _ = child.dispose().await;
                    active_children.lock().remove(&seq);
                    drop(permit);
                    panic!("workflow child result failed: {error}");
                }
            };
            let outcome = match settled.stop_reason {
                SubagentStopReason::Completed => WorkflowAgentOutcome::Completed,
                SubagentStopReason::Aborted => WorkflowAgentOutcome::Cancelled,
                _ => WorkflowAgentOutcome::Failed,
            };
            event_ctx.emit(
                "workflow/agent-end",
                vec![cordis::arc(WorkflowAgentEndInfo {
                    seq: info.seq,
                    label: info.label,
                    phase: info.phase,
                    child_id: info.child_id,
                    outcome,
                })],
            );
            let _ = child.dispose().await;
            active_children.lock().remove(&seq);
            drop(permit);
            if settled.stop_reason != SubagentStopReason::Completed {
                return Value::Null;
            }
            if call.schema.is_some() {
                return settled.structured.unwrap_or(Value::Null);
            }
            Value::String(output_text(&settled.output))
        })
    });
    let phase_ctx = ctx.clone();
    let phase: CodeBindingFunction = Arc::new(move |input| {
        let phase_ctx = phase_ctx.clone();
        Box::pin(async move {
            let title = input
                .as_array()
                .and_then(|args| args.first())
                .and_then(Value::as_str)
                .or_else(|| input.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if !title.is_empty() {
                phase_ctx.emit("workflow/phase", vec![cordis::arc(title)]);
            }
            Value::Null
        })
    });
    let args = request.args.unwrap_or(Value::Null);
    let program = format!(
        "const args = {};\nconst agent = (input) => workflowHost.agent(input);\nconst phase = (title) => workflowHost.phase(title);\n{}",
        serde_json::to_string(&args).expect("workflow args are JSON"),
        request.script,
    );
    let request_signal = request.signal.clone();
    let terminal_cancelled = cancelled.clone();
    let terminal_request_signal = request.signal.clone();
    let signal: CodeAbort = Arc::new(move || {
        cancelled.load(Ordering::Acquire) || request_signal.as_ref().is_some_and(|signal| signal())
    });
    match code_runtime
        .run(CodeRunRequest {
            program,
            bindings: vec![CodeBindingNamespace {
                global: "workflowHost".to_string(),
                functions: vec![("agent".to_string(), binding), ("phase".to_string(), phase)],
                error_class: None,
            }],
            signal: Some(signal),
        })
        .await
    {
        Ok(code)
            if code.error.is_none()
                && (terminal_cancelled.load(Ordering::Acquire)
                    || terminal_request_signal
                        .as_ref()
                        .is_some_and(|signal| signal())) =>
        {
            WorkflowResult::cancelled(
                Some("workflow cancelled".to_string()),
                agents_started.load(Ordering::Acquire),
            )
        }
        Ok(code) if code.error.is_none() => WorkflowResult {
            value: code.value.unwrap_or(Value::Null),
            stop_reason: dsh_workflow::WorkflowStopReason::Completed,
            error: None,
            agents_started: agents_started.load(Ordering::Acquire),
        },
        Ok(code) => {
            let failure = code.error.expect("guarded");
            WorkflowResult {
                value: Value::Null,
                stop_reason: if failure.kind == CodeRunFailureKind::Abort {
                    dsh_workflow::WorkflowStopReason::Cancelled
                } else {
                    dsh_workflow::WorkflowStopReason::Error
                },
                error: Some(failure.message),
                agents_started: agents_started.load(Ordering::Acquire),
            }
        }
        Err(error) => {
            let was_cancelled = terminal_cancelled.load(Ordering::Acquire)
                || terminal_request_signal
                    .as_ref()
                    .is_some_and(|signal| signal());
            WorkflowResult {
                value: Value::Null,
                stop_reason: if was_cancelled {
                    dsh_workflow::WorkflowStopReason::Cancelled
                } else {
                    dsh_workflow::WorkflowStopReason::Error
                },
                error: Some(if was_cancelled {
                    "workflow cancelled".to_string()
                } else {
                    error
                }),
                agents_started: agents_started.load(Ordering::Acquire),
            }
        }
    }
}

struct AgentInput {
    prompt: String,
    label: Option<String>,
    schema: Option<Value>,
}

fn read_agent_input(input: Value) -> AgentInput {
    let input = input
        .as_array()
        .and_then(|args| args.first())
        .cloned()
        .unwrap_or(input);
    match input {
        Value::String(prompt) => AgentInput {
            prompt,
            label: None,
            schema: None,
        },
        Value::Object(mut input) => AgentInput {
            prompt: input
                .remove("prompt")
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| panic!("agent input requires prompt")),
            label: input
                .remove("label")
                .and_then(|value| value.as_str().map(str::to_string)),
            schema: input.remove("schema"),
        },
        _ => panic!("agent input must be a prompt string or object"),
    }
}

fn output_text(output: &[ContentBlock]) -> String {
    output
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
}

fn validate_config(config: &Config) -> Result<(), WorkflowError> {
    if config.provider.trim().is_empty()
        || config.max_total_agents == 0
        || config.dispose_grace_ms == 0
    {
        return Err(WorkflowError::new(
            "workflow-node config values must be positive and provider non-empty",
            dsh_workflow::WorkflowErrorCode::InvalidArgument,
        ));
    }
    Ok(())
}
