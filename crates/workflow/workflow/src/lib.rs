//! Workflow orchestration capability seam and browser-safe lifecycle
//! vocabulary. Rust port of `@deepseek-ai/dsh-workflow`.

use std::any::Any;
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use cordis::{ArcValue, BoxFuture, Context, DispatchMode, Service, arc};
use dsh_agent::Agent;
use dsh_brand::Branded;
use dsh_llm::HarnessError;
use dsh_session::SessionId;
use futures::FutureExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[doc(hidden)]
pub enum WorkflowRunIdTag {}
pub type WorkflowRunId = Branded<WorkflowRunIdTag>;

pub fn workflow_run_id(id: impl Into<String>) -> WorkflowRunId {
    Branded::new(id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMeta {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<WorkflowPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStopReason {
    Completed,
    Cancelled,
    Error,
}

impl WorkflowStopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowResult {
    pub value: JsonValue,
    pub stop_reason: WorkflowStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub agents_started: u64,
}

impl WorkflowResult {
    pub fn completed(value: JsonValue, agents_started: u64) -> Self {
        Self {
            value,
            stop_reason: WorkflowStopReason::Completed,
            error: None,
            agents_started,
        }
    }

    pub fn cancelled(reason: Option<String>, agents_started: u64) -> Self {
        Self {
            value: JsonValue::Null,
            stop_reason: WorkflowStopReason::Cancelled,
            error: reason,
            agents_started,
        }
    }

    pub fn error(message: impl Into<String>, agents_started: u64) -> Self {
        Self {
            value: JsonValue::Null,
            stop_reason: WorkflowStopReason::Error,
            error: Some(message.into()),
            agents_started,
        }
    }

    pub fn info(&self) -> WorkflowResultInfo {
        WorkflowResultInfo {
            stop_reason: self.stop_reason,
            error: self.error.clone(),
            agents_started: self.agents_started,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunInfo {
    pub id: WorkflowRunId,
    pub meta: WorkflowMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgentInfo {
    pub seq: u64,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub child_id: SessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowAgentOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl WorkflowAgentOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgentEndInfo {
    pub seq: u64,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub child_id: SessionId,
    pub outcome: WorkflowAgentOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowResultInfo {
    pub stop_reason: WorkflowStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub agents_started: u64,
}

pub type AbortPredicate = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct WorkflowStartRequest {
    pub script: String,
    pub meta: WorkflowMeta,
    pub args: Option<JsonValue>,
    pub subagent_provider: Option<String>,
    pub max_total_agents: Option<u64>,
    pub parent: Arc<dyn Agent>,
    pub signal: Option<AbortPredicate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkflowErrorCode {
    ScriptParse,
    MetaInvalid,
    InvalidArgument,
    UnsupportedOption,
    UnsupportedSchema,
    AgentCap,
    ItemCap,
    AgentStart,
    AgentResult,
    ResultUnserializable,
    Cancelled,
}

impl WorkflowErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScriptParse => "SCRIPT_PARSE",
            Self::MetaInvalid => "META_INVALID",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::UnsupportedOption => "UNSUPPORTED_OPTION",
            Self::UnsupportedSchema => "UNSUPPORTED_SCHEMA",
            Self::AgentCap => "AGENT_CAP",
            Self::ItemCap => "ITEM_CAP",
            Self::AgentStart => "AGENT_START",
            Self::AgentResult => "AGENT_RESULT",
            Self::ResultUnserializable => "RESULT_UNSERIALIZABLE",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(Debug)]
pub struct WorkflowError {
    pub error: HarnessError,
    pub code: WorkflowErrorCode,
    pub fatal: bool,
}

impl WorkflowError {
    pub fn new(message: impl Into<String>, code: WorkflowErrorCode) -> Self {
        Self::with_fatal(message, code, true)
    }

    pub fn with_fatal(message: impl Into<String>, code: WorkflowErrorCode, fatal: bool) -> Self {
        Self {
            error: HarnessError::new(message, code.as_str()),
            code,
            fatal,
        }
    }

    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.error.message)
    }
}

impl Error for WorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.error.source()
    }
}

pub fn is_fatal_workflow_error(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<WorkflowError>()
        .is_some_and(|workflow| workflow.fatal)
}

pub fn validate_start_request(request: &WorkflowStartRequest) -> Result<(), WorkflowError> {
    if request.script.trim().is_empty() {
        return Err(WorkflowError::new(
            "workflow script must be a non-empty string",
            WorkflowErrorCode::ScriptParse,
        ));
    }
    if request.meta.name.trim().is_empty() {
        return Err(WorkflowError::new(
            "meta.name must be a non-empty string",
            WorkflowErrorCode::MetaInvalid,
        ));
    }
    if request.meta.description.trim().is_empty() {
        return Err(WorkflowError::new(
            "meta.description must be a non-empty string",
            WorkflowErrorCode::MetaInvalid,
        ));
    }
    if request.max_total_agents == Some(0) {
        return Err(WorkflowError::new(
            "maxTotalAgents must be positive",
            WorkflowErrorCode::InvalidArgument,
        ));
    }
    Ok(())
}

pub trait WorkflowRun: Send + Sync + 'static {
    fn id(&self) -> &WorkflowRunId;
    fn meta(&self) -> &WorkflowMeta;
    fn result(&self) -> BoxFuture<'static, WorkflowResult>;
    fn cancel(&self, reason: Option<String>);
    fn dispose(&self) -> BoxFuture<'static, ()>;
}

type CancelHook = Arc<dyn Fn(Option<String>) + Send + Sync>;
type CleanupHook = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

struct HolderState {
    result: Mutex<Option<WorkflowResult>>,
    result_notify: tokio::sync::Notify,
    cancel_started: AtomicBool,
    dispose_started: AtomicBool,
    dispose_done: AtomicBool,
    dispose_notify: tokio::sync::Notify,
}

pub struct HolderWorkflowRun {
    id: WorkflowRunId,
    meta: WorkflowMeta,
    grace: Duration,
    cancel_hook: CancelHook,
    cleanup_hook: CleanupHook,
    state: Arc<HolderState>,
}

#[derive(Clone)]
pub struct WorkflowRunController {
    state: Arc<HolderState>,
}

pub fn new_workflow_run(
    id: WorkflowRunId,
    meta: WorkflowMeta,
    grace: Duration,
    cancel_hook: CancelHook,
    cleanup_hook: CleanupHook,
) -> (Arc<HolderWorkflowRun>, WorkflowRunController) {
    let state = Arc::new(HolderState {
        result: Mutex::new(None),
        result_notify: tokio::sync::Notify::new(),
        cancel_started: AtomicBool::new(false),
        dispose_started: AtomicBool::new(false),
        dispose_done: AtomicBool::new(false),
        dispose_notify: tokio::sync::Notify::new(),
    });
    (
        Arc::new(HolderWorkflowRun {
            id,
            meta,
            grace,
            cancel_hook,
            cleanup_hook,
            state: state.clone(),
        }),
        WorkflowRunController { state },
    )
}

impl WorkflowRunController {
    pub fn settle(&self, result: WorkflowResult) -> bool {
        let mut slot = self.state.result.lock();
        if slot.is_some() {
            return false;
        }
        *slot = Some(result);
        drop(slot);
        self.state.result_notify.notify_waiters();
        true
    }
}

impl HolderWorkflowRun {
    async fn await_result(state: Arc<HolderState>) -> WorkflowResult {
        loop {
            let notified = state.result_notify.notified();
            if let Some(result) = state.result.lock().clone() {
                return result;
            }
            notified.await;
        }
    }

    async fn await_disposed(state: Arc<HolderState>) {
        loop {
            let notified = state.dispose_notify.notified();
            if state.dispose_done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub fn result(&self) -> BoxFuture<'static, WorkflowResult> {
        <Self as WorkflowRun>::result(self)
    }
    pub fn cancel(&self, reason: Option<String>) {
        <Self as WorkflowRun>::cancel(self, reason);
    }
    pub fn dispose(&self) -> BoxFuture<'static, ()> {
        <Self as WorkflowRun>::dispose(self)
    }
}

impl WorkflowRun for HolderWorkflowRun {
    fn id(&self) -> &WorkflowRunId {
        &self.id
    }
    fn meta(&self) -> &WorkflowMeta {
        &self.meta
    }
    fn result(&self) -> BoxFuture<'static, WorkflowResult> {
        Box::pin(Self::await_result(self.state.clone()))
    }

    fn cancel(&self, reason: Option<String>) {
        if self.state.result.lock().is_some()
            || self.state.cancel_started.swap(true, Ordering::AcqRel)
        {
            return;
        }
        (self.cancel_hook)(reason.clone());
        WorkflowRunController {
            state: self.state.clone(),
        }
        .settle(WorkflowResult::cancelled(reason, 0));
    }

    fn dispose(&self) -> BoxFuture<'static, ()> {
        if self.state.dispose_started.swap(true, Ordering::AcqRel) {
            return Box::pin(Self::await_disposed(self.state.clone()));
        }
        self.cancel(Some("disposed".to_string()));
        let cleanup = (self.cleanup_hook)();
        let grace = self.grace;
        let state = self.state.clone();
        Box::pin(async move {
            let _ = tokio::time::timeout(grace, cleanup).await;
            state.dispose_done.store(true, Ordering::Release);
            state.dispose_notify.notify_waiters();
        })
    }
}

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    Start(WorkflowRunInfo),
    Phase {
        run: WorkflowRunInfo,
        title: String,
    },
    Log {
        run: WorkflowRunInfo,
        message: String,
    },
    AgentStart {
        run: WorkflowRunInfo,
        agent: WorkflowAgentInfo,
    },
    AgentEnd {
        run: WorkflowRunInfo,
        agent: WorkflowAgentEndInfo,
    },
    End {
        run: WorkflowRunInfo,
        result: WorkflowResultInfo,
    },
}

impl WorkflowEvent {
    fn into_dispatch(self) -> (&'static str, Vec<ArcValue>) {
        match self {
            Self::Start(run) => ("workflow/start", vec![arc(run)]),
            Self::Phase { run, title } => ("workflow/phase", vec![arc(run), arc(title)]),
            Self::Log { run, message } => ("workflow/log", vec![arc(run), arc(message)]),
            Self::AgentStart { run, agent } => ("workflow/agent-start", vec![arc(run), arc(agent)]),
            Self::AgentEnd { run, agent } => ("workflow/agent-end", vec![arc(run), arc(agent)]),
            Self::End { run, result } => ("workflow/end", vec![arc(run), arc(result)]),
        }
    }
}

pub trait WorkflowEngine: Send + Sync + 'static {
    fn context(&self) -> &Context;
    fn start(&self, request: WorkflowStartRequest) -> Result<Arc<dyn WorkflowRun>, WorkflowError>;

    fn emit_workflow_event(&self, event: WorkflowEvent) {
        let (name, args) = event.into_dispatch();
        let ctx = self.context().clone();
        let listeners = ctx.collect(DispatchMode::Emit, name, &args);
        let logger = ctx.named_logger(Some("workflow"));
        let mut pending = Vec::new();
        for (listener_ctx, callback) in listeners {
            let future = catch_unwind(AssertUnwindSafe(|| callback(&listener_ctx, args.clone())));
            let mut future = match future {
                Ok(future) => future,
                Err(payload) => {
                    logger.warn(vec![arc(format!(
                        "workflow: {name} listener threw: {}",
                        render_panic(payload)
                    ))]);
                    continue;
                }
            };
            let waker = futures::task::noop_waker();
            let mut task_ctx = TaskContext::from_waker(&waker);
            match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(&mut task_ctx))) {
                Ok(Poll::Ready(_)) => {}
                Ok(Poll::Pending) => pending.push(future),
                Err(payload) => logger.warn(vec![arc(format!(
                    "workflow: {name} listener threw: {}",
                    render_panic(payload)
                ))]),
            }
        }
        for future in pending {
            let logger = logger.clone();
            let name = name.to_string();
            let task = async move {
                if let Err(payload) = AssertUnwindSafe(future).catch_unwind().await {
                    logger.warn(vec![arc(format!(
                        "workflow: {name} listener rejected: {}",
                        render_panic(payload)
                    ))]);
                }
            };
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(task);
                }
                Err(_) => {
                    std::thread::spawn(move || futures::executor::block_on(task));
                }
            }
        }
    }
}

impl Service for dyn WorkflowEngine {
    fn service_name(&self) -> &'static str {
        "workflowEngine"
    }
}

fn render_panic(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_string())
        })
        .unwrap_or_else(|| "[unrenderable thrown value]".to_string())
}
