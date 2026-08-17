//! Model-facing `job_output`, `job_list`, and `job_kill` tools over
//! `ctx.jobs`. Loading the plugin attaches the controller required by
//! producers. It also delivers unreported completions to the owning agent:
//! injected into a busy owner's next step, or opening a turn on an idle one
//! under the default `wakeup` delivery, bounded per owner. Rust port of
//! `packages/jobs/tool-jobs/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS synchronous `apply` is an async `apply` (the claimed-listener
//!   registration goes through the async `ctx.on`); the plugin body awaits
//!   it.
//! - `maxConsecutiveWakes` is a `u64`: JS `Infinity` is not representable
//!   in the JSON config space; fractional budgets are rejected while the
//!   config JSON is decoded.
//! - `JobRegistry::on_job_done`/`on_jobs_changed`/`attach_controller` take
//!   an explicit caller `Context` (the TS Proxy rebinding collapse), so the
//!   listener/controller scope and fiber follow the mounting composition.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, NextFn, Plugin, PluginError,
    ValidationError, arc, downcast, downcast_arc,
};
use dsh_agent::{Agent, AgentStatus};
use dsh_jobs::{JobId, JobRegistry, JobSnapshot, KillOutcome, job_id};
use dsh_llm::{
    ContentBlock, ContextForm, MessageSource, bound_context_summary, create_user_message,
};
use dsh_output_retention::{TextRetainer, TextRetentionStrategy};
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolExecution, ToolExecutionResult,
    ToolOutputDefinition, ToolRunContext, ToolRuntime,
};
use dsh_schemastery::{Data, Schema};

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "tool-jobs";

/// Services required before the plugin can attach tools and notices.
pub const INJECT: [&str; 3] = ["tools", "jobs", "systemPrompt"];

/// How an unreported completion reaches an owner that is already idle:
/// `wakeup` opens a turn for it, `quiet` leaves it pending until something
/// else wakes the owner. A busy owner is injected either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionDelivery {
    Quiet,
    Wakeup,
}

/// Configures bounded `job_output` waits and completion-notice delivery
/// (the TS `Config` interface; optionals default like the schema).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Wait duration applied when `job_output` sets `wait` without
    /// `timeout_ms` (default 30s).
    pub wait_timeout_ms: Option<u64>,
    /// Hard cap on any single wait; a larger model-supplied `timeout_ms` is
    /// clamped down to it (default 10min).
    pub max_wait_timeout_ms: Option<u64>,
    /// Whether a completion opens a turn on an idle owner (default
    /// `wakeup`).
    pub completion_delivery: Option<CompletionDelivery>,
    /// Turns one owner may have opened by completion wakes before the next
    /// notice degrades to injection, reset by any user-authored input
    /// (default 3).
    pub max_consecutive_wakes: Option<u64>,
}

/// The schema-defaulted configuration (TS `static Config`).
#[derive(Debug, Clone, Copy)]
pub struct ResolvedConfig {
    pub wait_timeout_ms: u64,
    pub max_wait_timeout_ms: u64,
    pub completion_delivery: CompletionDelivery,
    pub max_consecutive_wakes: u64,
}

impl Config {
    /// Apply the `??` fallbacks and the apply-time cross-field check (TS
    /// `apply`).
    pub fn resolve(&self) -> Result<ResolvedConfig, String> {
        let wait_timeout_ms = self.wait_timeout_ms.unwrap_or(30_000);
        let max_wait_timeout_ms = self.max_wait_timeout_ms.unwrap_or(600_000);
        if wait_timeout_ms > max_wait_timeout_ms {
            return Err(format!(
                "tool-jobs: waitTimeoutMs ({wait_timeout_ms}) exceeds maxWaitTimeoutMs ({max_wait_timeout_ms})"
            ));
        }
        Ok(ResolvedConfig {
            wait_timeout_ms,
            max_wait_timeout_ms,
            completion_delivery: self
                .completion_delivery
                .unwrap_or(CompletionDelivery::Wakeup),
            max_consecutive_wakes: self.max_consecutive_wakes.unwrap_or(3),
        })
    }
}

/// Task state safe for model-authored programs; ownership/bookkeeping
/// fields are omitted.
fn public_job(snapshot: &JobSnapshot) -> serde_json::Value {
    let mut job = serde_json::json!({
        "id": snapshot.id.as_str(),
        "kind": snapshot.kind,
        "label": snapshot.label,
        "status": snapshot.status.as_str(),
        "startedAt": snapshot.started_at,
    });
    if let Some(detail) = &snapshot.detail {
        job["detail"] = serde_json::json!(detail);
    }
    if let Some(finished_at) = snapshot.finished_at {
        job["finishedAt"] = serde_json::json!(finished_at);
    }
    job
}

/// Shared wire schema for job-control outputs (the status enum mirrors
/// [`dsh_jobs::JobStatus::as_str`]; the TS property-level `required: true`
/// annotations collapse into the standard root-level `required` array).
fn public_task_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string" },
            "kind": { "type": "string" },
            "label": { "type": "string" },
            "status": {
                "type": "string",
                "enum": ["running", "stopping", "completed", "killed", "failed"],
            },
            "detail": { "type": "string" },
            "startedAt": { "type": "integer" },
            "finishedAt": { "type": "integer" },
        },
        "required": ["id", "kind", "label", "status", "startedAt"],
    })
}

/// Render generic status with optional producer detail (TS `statusLine`).
pub fn status_line(snapshot: &JobSnapshot) -> String {
    match &snapshot.detail {
        Some(detail) => format!("[status: {}, {detail}]", snapshot.status.as_str()),
        None => format!("[status: {}]", snapshot.status.as_str()),
    }
}

/// The JSON-value form used while finalizing canonical output.
fn status_line_json(job: &serde_json::Value) -> String {
    match job.get("detail").and_then(|detail| detail.as_str()) {
        Some(detail) => format!(
            "[status: {}, {detail}]",
            job["status"].as_str().unwrap_or_default()
        ),
        None => format!("[status: {}]", job["status"].as_str().unwrap_or_default()),
    }
}

fn retain_tail(text: &str, max_bytes: u64) -> String {
    let mut retainer = TextRetainer::new(TextRetentionStrategy::Tail {
        max_bytes: max_bytes as usize,
    });
    retainer.push(text.as_bytes());
    retainer.finish().text
}

fn retain_head(text: &str, max_bytes: u64) -> String {
    let mut retainer = TextRetainer::new(TextRetentionStrategy::Head {
        max_bytes: max_bytes as usize,
    });
    retainer.push(text.as_bytes());
    retainer.finish().text
}

fn fit_with_suffix(
    content: &str,
    suffix: &str,
    max_bytes: Option<u64>,
    omitted: &str,
) -> String {
    let complete = format!("{content}{suffix}");
    if max_bytes.is_none_or(|max| complete.as_bytes().len() as u64 <= max) {
        return complete;
    }
    let max = max_bytes.expect("checked above");
    let fixed = format!(
        "{}{suffix}",
        if content.ends_with(omitted.trim_start()) {
            ""
        } else {
            omitted
        }
    );
    let fixed_bytes = fixed.as_bytes().len() as u64;
    if fixed_bytes >= max {
        return retain_tail(&fixed, max);
    }
    format!("{}{fixed}", retain_tail(content, max - fixed_bytes))
}

/// One-line account of a settled job for the `notice` form's collapsed row.
fn completion_summary(snapshot: &JobSnapshot) -> String {
    bound_context_summary(&format!(
        "{} {} {}",
        snapshot.kind,
        snapshot.label,
        status_line(snapshot)
    ))
}

/// The completion notice, bounded by the producer's output budget while
/// preserving the job id and the collection action.
fn fit_completion_notice(snapshot: &JobSnapshot) -> String {
    let prefix = format!("background job {}", snapshot.id.as_str());
    let detail = format!(
        " ({}: {}) finished {}",
        snapshot.kind,
        snapshot.label,
        status_line(snapshot)
    );
    let action = "\nDone; job_output.";
    let complete = format!("{prefix}{detail}. Read its output with job_output.");
    let Some(max_bytes) = snapshot.output_limit_bytes else {
        return complete;
    };
    if complete.as_bytes().len() as u64 <= max_bytes {
        return complete;
    }
    let omitted = "\n[notice truncated]";
    let fixed = format!("{prefix}{omitted}{action}");
    let fixed_bytes = fixed.as_bytes().len() as u64;
    if fixed_bytes <= max_bytes {
        return if fixed_bytes == max_bytes {
            fixed
        } else {
            format!(
                "{prefix}{}{omitted}{action}",
                retain_head(&detail, max_bytes - fixed_bytes)
            )
        };
    }
    let compact = format!("{prefix}{action}");
    let compact_bytes = compact.as_bytes().len() as u64;
    if compact_bytes <= max_bytes {
        return compact;
    }
    let action_bytes = action.as_bytes().len() as u64;
    if action_bytes >= max_bytes {
        return retain_tail(action, max_bytes);
    }
    format!("{}{action}", retain_head(&prefix, max_bytes - action_bytes))
}

fn raw_single_text(content: &[ContentBlock]) -> Option<&str> {
    match content {
        [ContentBlock::Text { text }] => Some(text),
        _ => None,
    }
}

fn bound_single_text(
    content: &[ContentBlock],
    max_bytes: u64,
) -> Option<Vec<ContentBlock>> {
    let text = raw_single_text(content)?;
    Some(vec![ContentBlock::Text {
        text: fit_with_suffix(text, "", Some(max_bytes), "\n[result truncated]"),
    }])
}

/// Validate the non-empty constraint that the JSON Schema subset cannot
/// express.
fn validate_job_id(value: Option<&str>) -> Result<JobId, ToolBodyError> {
    let value = value.ok_or_else(|| ToolBodyError::plain("invalid job_id: expected a string"))?;
    if value.is_empty() {
        return Err(ToolBodyError::plain(format!(
            "invalid job_id: expected a non-empty string, got {value:?}"
        )));
    }
    Ok(job_id(value))
}

/// Pending presentation shared by the three generic job controls.
fn present_task_call(
    title: String,
    kind: ToolCallKind,
    raw_input: Option<serde_json::Value>,
) -> Option<ToolCallView> {
    Some(ToolCallView::Generic {
        title,
        kind: Some(kind),
        raw_input,
        content: None,
        locations: None,
    })
}

/// The mounted service: tools, controller, and completion delivery.
pub struct ToolJobsService {
    ctx: Context,
    jobs: Arc<dyn JobRegistry>,
    config: ResolvedConfig,
    /// Per-execution output caps keyed by the registry execution token (the
    /// TS `WeakMap<ToolExecution, number>` identity collapse).
    output_limits: Arc<parking_lot::Mutex<HashMap<u64, u64>>>,
    /// Turns this plugin opened per exact owner since that owner last
    /// consumed human input (the TS `WeakMap<Agent, number>` collapse).
    spent_wakes: Arc<parking_lot::Mutex<HashMap<usize, u64>>>,
}

impl ToolJobsService {
    /// Mount the tools, controller, and listeners (TS `apply`).
    pub async fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let resolved = config.resolve()?;
        let jobs = ctx
            .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-tool-jobs requires the jobs service".to_string())?;
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-tool-jobs requires the tools service".to_string())?;
        let prompt = ctx
            .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "dsh-tool-jobs requires the systemPrompt service".to_string())?;

        let service = Arc::new(Self {
            ctx: ctx.clone(),
            jobs,
            config: resolved,
            output_limits: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            spent_wakes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        });

        // Claiming is the point the human's input actually enters a step; a
        // notice this plugin itself queued must not refill the budget it
        // just spent.
        if resolved.completion_delivery == CompletionDelivery::Wakeup {
            let spent_wakes = service.spent_wakes.clone();
            let claimed: Arc<Listener> = Arc::new(move |_ctx, args| {
                let spent_wakes = spent_wakes.clone();
                Box::pin(async move {
                    if let Some(payload) = args
                        .first()
                        .and_then(|value| downcast::<dsh_agent::AgentInboxClaimedPayload>(value))
                    {
                        if matches!(payload.message.source, MessageSource::User { .. }) {
                            let key = Arc::as_ptr(&payload.agent).cast::<()>() as usize;
                            spent_wakes.lock().remove(&key);
                        }
                    }
                    None
                })
            });
            ctx.on("agent/inbox/claimed", claimed, Default::default()).await;
        }

        // Capture the producer output cap before policy runs, so every
        // policy stage's content is bounded.
        let pre_execute_service = service.clone();
        let pre_execute: Arc<Listener> = Arc::new(move |_ctx, args| {
            let service = pre_execute_service.clone();
            Box::pin(async move {
                if let Some(exec) = args
                    .first()
                    .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
                    .map(|slot| slot.as_ref().clone())
                {
                    if let Some(max_bytes) = service.visible_output_limit(&exec) {
                        service.output_limits.lock().insert(exec.token, max_bytes);
                    }
                }
                let Some(next) = args
                    .last()
                    .and_then(|value| downcast_arc::<NextFn>(value))
                else {
                    return Some(arc(()));
                };
                let value = next.call().await;
                Some(value)
            })
        });
        ctx.on(
            "tools/pre-execute",
            pre_execute,
            EventOptions::default().prepend(true),
        )
        .await;

        // Producers may start work only while a controller is attached.
        let controller = service.jobs.attach_controller(ctx, "tool-jobs");
        let controller_for_effect = controller.clone();
        let _ = ctx.effect(
            "tool-jobs.controller()",
            Box::pin(async move { Some(controller_for_effect) }),
        );

        // Cross-call guidance follows the bash section and precedes product
        // sections.
        prompt.section(
            ctx,
            PromptSection {
                name: "tool:jobs".to_string(),
                order: 106.0,
                text: PromptText::Static(
                    "Track every background job id you start. You are notified in-session when a job finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running job's work. Before giving a final answer, collect every still-relevant job with job_output (set wait: true only when you are genuinely blocked on it), and job_kill jobs that stopped mattering.".to_string(),
                ),
                complete: None,
            },
        );

        // Deliver unreported completions to the exact lifecycle owner.
        let done_service = service.clone();
        let done_listener: dsh_jobs::JobDoneListener = Arc::new(
            move |snapshot: JobSnapshot, owner: Option<Arc<dyn Agent>>| {
                if snapshot.reported {
                    return;
                }
                let Some(owner) = owner else {
                    return;
                };
                let message = create_user_message(
                    vec![ContentBlock::Text {
                        text: fit_completion_notice(&snapshot),
                    }],
                    MessageSource::Plugin {
                        plugin: NAME.to_string(),
                        form: Some(ContextForm::Notice),
                        sections: None,
                        summary: Some(completion_summary(&snapshot)),
                        compaction_id: None,
                        source_command_id: None,
                    },
                );
                let key = Arc::as_ptr(&owner).cast::<()>() as usize;
                let spent = done_service
                    .spent_wakes
                    .lock()
                    .get(&key)
                    .copied()
                    .unwrap_or(0);
                if done_service.config.completion_delivery == CompletionDelivery::Wakeup
                    && owner.status() == AgentStatus::Idle
                    && spent < done_service.config.max_consecutive_wakes
                {
                    done_service.spent_wakes.lock().insert(key, spent + 1);
                    owner.followup(message);
                    return;
                }
                owner.inject(message);
            },
        );
        service.jobs.on_job_done(ctx, done_listener);

        tools.register(ctx, job_output_definition(service.clone()))?;
        tools.register(ctx, job_list_definition(service.clone()))?;
        tools.register(ctx, job_kill_definition(service.clone()))?;
        Ok(service)
    }

    /// The producer cap that applies to one pending control call.
    fn visible_output_limit(&self, exec: &ToolExecution) -> Option<u64> {
        if exec.name != "job_output" && exec.name != "job_kill" {
            return None;
        }
        let job_id = exec.arguments.get("job_id")?.as_str()?;
        if job_id.is_empty() {
            return None;
        }
        self.jobs
            .list(exec.agent.as_ref())
            .into_iter()
            .find(|snapshot| snapshot.id.as_str() == job_id)
            .and_then(|snapshot| snapshot.output_limit_bytes)
    }

    /// The definition-owned last-mile content transform.
    fn finalize_task_content(
        &self,
        exec: &ToolExecution,
        result: &ToolExecutionResult,
    ) -> Option<Vec<ContentBlock>> {
        let max_bytes = {
            let mut limits = self.output_limits.lock();
            limits
                .remove(&exec.token)
                .or_else(|| self.visible_output_limit(exec))
        };
        let Some(max_bytes) = max_bytes else {
            return None;
        };
        if exec.name == "job_output" && !result.is_error {
            // This definition owns and schema-validates the canonical value.
            // Preserve its output/status split only while policy left the
            // default rendering intact.
            let value = result.value.as_ref()?;
            let text = value.get("text")?.as_str()?;
            let job = &value["job"];
            let body = if text.is_empty() { "(no new output)" } else { text };
            let content = body.strip_suffix('\n').unwrap_or(body);
            let suffix = format!("\n{}", status_line_json(job));
            if raw_single_text(&result.content) == Some(format!("{content}{suffix}").as_str()) {
                return Some(vec![ContentBlock::Text {
                    text: fit_with_suffix(content, &suffix, Some(max_bytes), "\n[output truncated]"),
                }]);
            }
        }
        bound_single_text(&result.content, max_bytes)
    }
}

fn job_output_definition(service: Arc<ToolJobsService>) -> ToolDefinition {
    let execute_service = service.clone();
    let finalize_service = service.clone();
    ToolDefinition {
        name: "job_output".to_string(),
        description: "Read a background job. Stream jobs return only output since the previous read; final-output jobs return their result after settlement. Every response ends with `[status: ...]`. Reads are non-blocking unless `wait: true`, which waits up to the configured cap."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "job_id": { "type": "string", "required": true, "description": "Job id returned by the tool that started the background work." },
                "wait": { "type": "boolean", "description": "Block until the job reaches a terminal status or the timeout expires. A timed-out wait returns [status: running] and leaves the job alive." },
                "timeout_ms": { "type": "number", "description": "Max wait in milliseconds (only meaningful with wait: true). Defaults to the configured wait timeout; capped by the configured maximum." },
            },
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "text": { "type": "string" },
                    "job": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": { "type": "string" },
                            "kind": { "type": "string" },
                            "label": { "type": "string" },
                            "status": { "type": "string", "enum": ["running", "stopping", "completed", "killed", "failed"] },
                            "detail": { "type": "string" },
                            "startedAt": { "type": "integer" },
                            "finishedAt": { "type": "integer" },
                        },
                        "required": ["id", "kind", "label", "status", "startedAt"],
                    },
                },
                "required": ["text", "job"],
            }),
            render: Arc::new(|_args, value| {
                let body = value["text"].as_str().unwrap_or_default();
                let body = if body.is_empty() { "(no new output)" } else { body };
                let separator = if body.ends_with('\n') { "" } else { "\n" };
                Ok(vec![ContentBlock::Text {
                    text: format!("{body}{separator}{}", status_line_json(&value["job"])),
                }])
            }),
            presentation_meta: None,
        },
        // A timed-out wait returns job state rather than a TOOL_TIMEOUT
        // error, so this tool owns its deadline instead of using
        // ToolDefinition.timeoutMs.
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args: &serde_json::Value, ctx: &ToolRunContext| {
            let service = execute_service.clone();
            let agent = ctx.execution.agent.clone();
            let signal = ctx.execution.signal.lock().clone();
            let args = args.clone();
            Box::pin(async move {
                let id = validate_job_id(args.get("job_id").and_then(|v| v.as_str()))?;
                if args.get("wait") == Some(&serde_json::Value::Bool(true)) {
                    let timeout = args
                        .get("timeout_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(service.config.wait_timeout_ms)
                        .min(service.config.max_wait_timeout_ms);
                    service
                        .jobs
                        .wait(&id, timeout, agent.as_ref(), Some(signal))
                        .await
                        .map_err(ToolBodyError::plain)?;
                }
                let read = service
                    .jobs
                    .read(&id, agent.as_ref())
                    .map_err(ToolBodyError::plain)?;
                Ok(serde_json::json!({
                    "text": read.text,
                    "job": public_job(&read.snapshot),
                }))
            })
        }),
        finalize_content: Some(Arc::new(move |exec, result| {
            finalize_service.finalize_task_content(exec, result)
        })),
        present_call: Some(Arc::new(|args| {
            let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or_default();
            present_task_call(
                format!("Read output from background job {job_id}"),
                ToolCallKind::Read,
                args.get("job_id").cloned(),
            )
        })),
        present_result: None,
    }
}

fn job_list_definition(service: Arc<ToolJobsService>) -> ToolDefinition {
    let list_service = service;
    ToolDefinition {
        name: "job_list".to_string(),
        description:
            "List your background jobs (running and finished) with their ids, kinds, and statuses."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({ "type": "array", "items": public_task_schema() }),
            render: Arc::new(|_args, jobs| {
                let jobs = jobs.as_array().cloned().unwrap_or_default();
                if jobs.is_empty() {
                    return Ok(vec![ContentBlock::Text {
                        text: "(no background jobs)".to_string(),
                    }]);
                }
                let lines: Vec<String> = jobs
                    .iter()
                    .map(|job| {
                        format!(
                            "{} [{}] {} — {}",
                            job["id"].as_str().unwrap_or_default(),
                            job["kind"].as_str().unwrap_or_default(),
                            job["status"].as_str().unwrap_or_default(),
                            job["label"].as_str().unwrap_or_default(),
                        )
                    })
                    .collect();
                Ok(vec![ContentBlock::Text {
                    text: lines.join("\n"),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |_args: &serde_json::Value, ctx: &ToolRunContext| {
            let service = list_service.clone();
            let agent = ctx.execution.agent.clone();
            Box::pin(async move {
                let jobs = service
                    .jobs
                    .list(agent.as_ref())
                    .iter()
                    .map(public_job)
                    .collect::<Vec<_>>();
                Ok(serde_json::Value::Array(jobs))
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|_args| {
            present_task_call("List background jobs".to_string(), ToolCallKind::Read, None)
        })),
        present_result: None,
    }
}

fn job_kill_definition(service: Arc<ToolJobsService>) -> ToolDefinition {
    let kill_service = service.clone();
    let finalize_service = service;
    ToolDefinition {
        name: "job_kill".to_string(),
        description: "Request cancellation of a running background job by job id. Returns immediately; the job settles as killed once its work actually stops."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "job_id": { "type": "string", "required": true, "description": "Job id returned by the tool that started the background work." },
                "reason": { "type": "string", "description": "Optional short reason, recorded in the log and forwarded to the job." },
            },
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "outcome": { "type": "string", "enum": ["cancellation-requested", "already-finished"] },
                    "job": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": { "type": "string" },
                            "kind": { "type": "string" },
                            "label": { "type": "string" },
                            "status": { "type": "string", "enum": ["running", "stopping", "completed", "killed", "failed"] },
                            "detail": { "type": "string" },
                            "startedAt": { "type": "integer" },
                            "finishedAt": { "type": "integer" },
                        },
                        "required": ["id", "kind", "label", "status", "startedAt"],
                    },
                },
                "required": ["outcome", "job"],
            }),
            render: Arc::new(|_args, value| {
                let job = &value["job"];
                let text = if value["outcome"] == "already-finished" {
                    format!(
                        "job {} had already finished {}",
                        job["id"].as_str().unwrap_or_default(),
                        status_line_json(job)
                    )
                } else {
                    format!(
                        "requested cancellation of job {}",
                        job["id"].as_str().unwrap_or_default()
                    )
                };
                Ok(vec![ContentBlock::Text { text }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args: &serde_json::Value, ctx: &ToolRunContext| {
            let service = kill_service.clone();
            let agent = ctx.execution.agent.clone();
            let args = args.clone();
            Box::pin(async move {
                let id = validate_job_id(args.get("job_id").and_then(|v| v.as_str()))?;
                let reason = args
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|reason| reason.to_string());
                let outcome = service
                    .jobs
                    .kill(&id, agent.as_ref(), reason)
                    .map_err(ToolBodyError::plain)?;
                // A snapshot describes current state without consuming
                // pending output.
                let snapshot = service
                    .jobs
                    .get(&id, agent.as_ref())
                    .map_err(ToolBodyError::plain)?;
                Ok(serde_json::json!({
                    "outcome": match outcome {
                        KillOutcome::AlreadyFinished => "already-finished",
                        KillOutcome::Requested => "cancellation-requested",
                    },
                    "job": public_job(&snapshot),
                }))
            })
        }),
        finalize_content: Some(Arc::new(move |exec, result| {
            finalize_service.finalize_task_content(exec, result)
        })),
        present_call: Some(Arc::new(|args| {
            let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or_default();
            present_task_call(
                format!("Kill background job {job_id}"),
                ToolCallKind::Execute,
                args.get("job_id").cloned(),
            )
        })),
        present_result: None,
    }
}

/// Mount the tools, controller, and listeners (TS `apply`).
pub async fn apply(ctx: &Context, config: Config) -> Result<(), String> {
    ToolJobsService::install(ctx, config).await.map(|_| ())
}

/// The Cordis plugin form (TS mounts the module with its schema; the config
/// arrives as the validated JSON value).
pub struct ToolJobsPlugin;

impl ToolJobsPlugin {
    pub fn new() -> Self {
        Self
    }
}

/// Read a whole-number config field (schema defaults arrive as `f64` JSON
/// numbers; `as_u64` alone would reject `30000.0`). The bound mirrors the TS
/// `Number.isSafeInteger` checks these budgets exist to bound.
fn whole_number(value: &serde_json::Value) -> Option<u64> {
    if let Some(integer) = value.as_u64() {
        return Some(integer);
    }
    let number = value.as_f64()?;
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 {
        return None;
    }
    if number > 9_007_199_254_740_991.0 {
        return None;
    }
    Some(number as u64)
}

/// Decode one plugin config from its validated JSON value.
fn config_from_value(config: &ArcValue) -> Result<Config, String> {
    let Some(value) = downcast::<serde_json::Value>(config) else {
        return Ok(Config::default());
    };
    let wait_timeout_ms = value
        .get("waitTimeoutMs")
        .map(|v| {
            whole_number(v).ok_or_else(|| {
                "tool-jobs: waitTimeoutMs must be a whole number of milliseconds".to_string()
            })
        })
        .transpose()?;
    let max_wait_timeout_ms = value
        .get("maxWaitTimeoutMs")
        .map(|v| {
            whole_number(v).ok_or_else(|| {
                "tool-jobs: maxWaitTimeoutMs must be a whole number of milliseconds".to_string()
            })
        })
        .transpose()?;
    let completion_delivery = value
        .get("completionDelivery")
        .map(|v| match v.as_str() {
            Some("quiet") => Ok(CompletionDelivery::Quiet),
            Some("wakeup") => Ok(CompletionDelivery::Wakeup),
            _ => Err("tool-jobs: completionDelivery must be \"quiet\" or \"wakeup\"".to_string()),
        })
        .transpose()?;
    let max_consecutive_wakes = value
        .get("maxConsecutiveWakes")
        .map(|v| {
            whole_number(v)
                .ok_or_else(|| "tool-jobs: maxConsecutiveWakes must be a whole number of turns".to_string())
        })
        .transpose()?;
    Ok(Config {
        wait_timeout_ms,
        max_wait_timeout_ms,
        completion_delivery,
        max_consecutive_wakes,
    })
}

/// The plugin-config schema (TS `static Config`).
fn config_schema() -> Schema {
    Schema::object(indexmap::IndexMap::from([
        (
            "waitTimeoutMs".to_string(),
            Schema::number().min(1.0).default(Data::Number(30_000.0)),
        ),
        (
            "maxWaitTimeoutMs".to_string(),
            Schema::number().min(1.0).default(Data::Number(600_000.0)),
        ),
        (
            "completionDelivery".to_string(),
            Schema::union(vec![
                Schema::constant(Data::String("quiet".to_string())),
                Schema::constant(Data::String("wakeup".to_string())),
            ])
            .default(Data::String("wakeup".to_string())),
        ),
        (
            "maxConsecutiveWakes".to_string(),
            Schema::number().min(1.0).default(Data::Number(3.0)),
        ),
    ]))
}

fn data_from_json(value: &serde_json::Value) -> Data {
    match value {
        serde_json::Value::Null => Data::Null,
        serde_json::Value::Bool(value) => Data::Bool(*value),
        serde_json::Value::Number(value) => {
            Data::Number(value.as_f64().unwrap_or(f64::NAN))
        }
        serde_json::Value::String(value) => Data::String(value.clone()),
        serde_json::Value::Array(items) => {
            Data::Array(items.iter().map(data_from_json).collect())
        }
        serde_json::Value::Object(map) => Data::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), data_from_json(value)))
                .collect(),
        ),
    }
}

#[async_trait::async_trait]
impl Plugin for ToolJobsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    fn validate(&self, config: ArcValue) -> Result<ArcValue, ValidationError> {
        let Some(value) = downcast::<serde_json::Value>(&config) else {
            return Ok(config);
        };
        let data = data_from_json(value);
        let validated = Schema::validate(&config_schema(), data)
            .map_err(|error| ValidationError::new([error.to_string()]))?;
        let json = validated
            .to_json()
            .unwrap_or_else(|| serde_json::Value::Null);
        Ok(arc(json))
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config_from_value(&config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        apply(ctx, config)
            .await
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
