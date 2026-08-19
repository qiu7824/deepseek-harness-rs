//! Model-facing delegation through one configured `ctx.subagents` provider.
//! Provider lifecycle controls tool registration and context-sensitive
//! schema wording. Foreground calls always dispose the run after
//! collection; one-shot background calls own a plain Task. Rust port of
//! `packages/subagent/tool-subagent/src/index.ts`.
//!
//! # Deviations
//!
//! - `backgroundMode: 'continuable'` is rejected at mount until the
//!   continuation manager lands (`CONTINUATION_UNAVAILABLE`).
//! - The provider-added/removed listeners re-mount the tool the same way
//!   the TS apply does; registration runs on the calling context.

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_agent::{Agent, AgentOptions};
use dsh_jobs::{JobOutcome, JobOutcomeStatus, JobRegistry, JobStart};
use dsh_llm::ContentBlock;
use dsh_subagent::{
    SubagentProvider, SubagentResult, SubagentRun, SubagentRuntime, SubagentStartRequest,
    SubagentStopReason, settle_run,
};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime,
};

/// Cordis plugin name.
pub const NAME: &str = "tool-subagent";

/// Services required before the tool can register.
pub const INJECT: [&str; 3] = ["tools", "subagents", "systemPrompt"];

/// Config: which registered provider this tool delegates to, plus child
/// defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// The `ctx.subagents` provider name to start runs on (e.g. `spawn`).
    pub provider: String,
    /// Model-facing tool name (default `subagent`).
    pub tool_name: Option<String>,
    /// Expose `run_in_background` (default true).
    pub enable_run_in_background: Option<bool>,
    /// Background execution policy (default `one-shot`).
    pub background_mode: Option<String>,
    /// Agent options applied to every child.
    pub agent_options: Option<AgentOptions>,
    /// Per-child persona that shadows `deployment:persona`.
    pub persona: Option<String>,
    /// Tool filter applied to every child.
    pub tool_filter: Option<dsh_tools::ToolRestriction>,
    /// Maximum child depth (default `3`), or `provider-managed`.
    pub max_depth: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: String::new(),
            tool_name: None,
            enable_run_in_background: None,
            background_mode: None,
            agent_options: None,
            persona: None,
            tool_filter: None,
            max_depth: Some(3),
        }
    }
}

/// Render text blocks from the canonical JSON block array without trusting
/// arbitrary values.
fn output_value_text(values: &[serde_json::Value]) -> String {
    values
        .iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            if object.get("type")?.as_str()? != "text" {
                return None;
            }
            Some(object.get("text")?.as_str()?.to_string())
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A non-`completed` stop reason means the child did not finish cleanly.
fn stop_reason_error(result: &SubagentResult) -> Option<String> {
    match result.stop_reason {
        SubagentStopReason::Completed => None,
        SubagentStopReason::Aborted => Some("subagent run was cancelled".to_string()),
        SubagentStopReason::Error => Some("subagent run failed".to_string()),
        SubagentStopReason::MaxTokens => {
            Some("subagent run hit its token limit before finishing".to_string())
        }
        SubagentStopReason::Refusal => Some("subagent declined the task".to_string()),
    }
}

/// Append the child's preserved partial answer to a stop-reason error.
fn with_partial_text(error: &str, output: &[ContentBlock]) -> String {
    let text = output
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        error.to_string()
    } else {
        format!("{error}\nPartial output before the run ended:\n{text}")
    }
}

/// Collect and release one foreground run without letting disposal replace
/// an independent result failure.
async fn settle_foreground_run(run: Arc<dyn SubagentRun>) -> Result<serde_json::Value, String> {
    let execution = run.result().await.and_then(|result| {
        if let Some(error) = stop_reason_error(&result) {
            return Err(with_partial_text(&error, &result.output));
        }
        Ok(serde_json::json!({
            "kind": "foreground",
            "runId": run.id().as_str(),
            "output": result.output,
        }))
    });
    let disposal = run.dispose().await;
    match (execution, disposal) {
        (Err(error), Err(dispose_error)) => Err(format!(
            "subagent run failed: {error}; dispose failed: {dispose_error}"
        )),
        (Err(error), Ok(())) => Err(error),
        (Ok(_value), Err(dispose_error)) => Err(format!("dispose failed: {dispose_error}")),
        (Ok(value), Ok(())) => Ok(value),
    }
}

/// Model-facing wording from the provider's conversation-history descriptor.
fn provider_wording(inherits_conversation: bool) -> (String, String) {
    if inherits_conversation {
        (
            "Delegate a task to a subagent that inherits this conversation: a child agent seeded with all completed turns so far (it does not see the current in-flight turn). Use this when the subtask builds on this conversation's context — a follow-up analysis, a review, a continuation — without consuming this conversation's context for the work itself. You receive its result, not its intermediate steps."
                .to_string(),
            "The task for the subagent. It already sees this conversation's completed turns, so build on them freely and state only what is new."
                .to_string(),
        )
    } else {
        (
            "Delegate a self-contained task to a subagent (a separate agent that works in its own context) to offload focused, independent work — research, a scoped implementation, an analysis — so it does not consume this conversation's context. The subagent returns its result, not its intermediate steps. Give it a complete, standalone prompt: it does not see this conversation."
                .to_string(),
            "The complete, self-contained task for the subagent. It does not share this conversation's context, so include everything it needs."
                .to_string(),
        )
    }
}

/// Install the tool, mirroring the provider lifecycle (TS `apply`).
pub fn apply(ctx: &Context, config: &Config) -> Result<(), String> {
    let background_enabled = config.enable_run_in_background.unwrap_or(true);
    let continuable = config.background_mode.as_deref() == Some("continuable");
    if continuable {
        return Err(
            "tool-subagent: backgroundMode continuable requires the continuation manager (not ported)"
                .to_string(),
        );
    }
    let tool_name = config
        .tool_name
        .clone()
        .unwrap_or_else(|| "subagent".to_string());
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent requires the tools service".to_string())?;
    let subagents = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent requires the subagents service".to_string())?;

    let Some(provider) = subagents.get_provider(&config.provider) else {
        return Err(format!(
            "subagent provider \"{}\" not registered yet; the tool will not register",
            config.provider
        ));
    };
    mount_tool(
        ctx,
        &tools,
        &subagents,
        provider,
        config,
        background_enabled,
        &tool_name,
    )
}

fn mount_tool(
    ctx: &Context,
    tools: &Arc<ToolRuntime>,
    subagents: &Arc<SubagentRuntime>,
    provider: Arc<dyn SubagentProvider>,
    config: &Config,
    background_enabled: bool,
    tool_name: &str,
) -> Result<(), String> {
    if let Some(max_depth) = config.max_depth {
        if !provider.capabilities().depth_limit {
            return Err(format!(
                "tool-subagent: provider \"{}\" cannot enforce maxDepth (no depthLimit capability) — set maxDepth: 'provider-managed' to leave the recursion budget to the provider",
                provider.name()
            ));
        }
        dsh_subagent::assert_subagent_max_depth(Some(max_depth))
            .map_err(|message| format!("tool-subagent: {message}"))?;
    }
    let (description, prompt_description) = provider_wording(provider.inherits_parent_context());
    let description = description
        + if background_enabled {
            " This call waits for the result by default. Set `run_in_background: true` to return a job id; collect with `job_output` and stop with `job_kill`."
        } else {
            " This call waits for the subagent and returns its result."
        };

    let ctx_for_tool = ctx.clone();
    let provider_for_tool = config.provider.clone();
    let config_for_tool = config.clone();
    let subagents_for_tool = subagents.clone();
    let definition = ToolDefinition {
        name: tool_name.to_string(),
        description,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 word) description of the delegated task, for display."
                },
                "prompt": {
                    "type": "string",
                    "description": prompt_description
                }
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "const": "background" },
                            "jobId": { "type": "string" }
                        },
                        "required": ["kind", "jobId"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "const": "foreground" },
                            "runId": { "type": "string" },
                            "output": { "type": "array" }
                        },
                        "required": ["kind", "runId", "output"]
                    }
                ]
            }),
            render: Arc::new(|_args, value| {
                let text = match value["kind"].as_str() {
                    Some("background") => format!(
                        "started background subagent task {}",
                        value["jobId"].as_str().unwrap_or("")
                    ),
                    _ => output_value_text(value["output"].as_array().unwrap_or(&vec![])),
                };
                Ok(vec![ContentBlock::Text { text }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: Some(Arc::new(|_args| true)),
        execute: Arc::new(move |args, exec| {
            let args = args.clone();
            let agent = exec.agent.clone();
            let signal = exec.signal.lock().clone();
            let ctx = ctx_for_tool.clone();
            let provider = provider_for_tool.clone();
            let config = config_for_tool.clone();
            let subagents = subagents_for_tool.clone();
            let background_enabled = background_enabled;
            Box::pin(async move {
                let Some(parent) = agent else {
                    return Err(ToolBodyError::plain(
                        "subagent tool requires a calling agent (exec.agent was undefined)",
                    ));
                };
                let request = SubagentStartRequest {
                    label: args
                        .get("description")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    prompt: vec![ContentBlock::Text {
                        text: args
                            .get("prompt")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    parent: parent.clone(),
                    signal,
                    agent_options: config.agent_options.clone(),
                    output_schema: None,
                    max_depth: config.max_depth,
                    tool_filter: config.tool_filter.clone(),
                    persona: config.persona.clone(),
                };
                let run_in_background = args
                    .get("run_in_background")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if run_in_background {
                    if !background_enabled {
                        return Err(ToolBodyError::plain(
                            "run_in_background is disabled for this tool instance (enableRunInBackground: false)",
                        ));
                    }
                    let Some(jobs) = ctx
                        .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return Err(ToolBodyError::plain(
                            "background jobs unavailable: load @deepseek-ai/dsh-jobs and @deepseek-ai/dsh-tool-jobs",
                        ));
                    };
                    let label = args
                        .get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or("subagent")
                        .to_string();
                    let start_subagents = subagents.clone();
                    let start_provider = provider.clone();
                    let start_request = request;
                    let id = jobs
                        .start(JobStart {
                            kind: "subagent".to_string(),
                            label: label.clone(),
                            output_limit_bytes: None,
                            owner: Some(parent),
                            run: Arc::new({
                                let subagents_owned = start_subagents.clone();
                                let provider_owned = start_provider.clone();
                                let request_owned = start_request.clone();
                                move || {
                                    let subagents = subagents_owned.clone();
                                    let provider = provider_owned.clone();
                                    let request = request_owned.clone();
                                    Arc::new(SettledRunHooks::new(Box::pin(async move {
                                        subagents.start(&provider, request).await
                                    })))
                                }
                            }),
                        })
                        .map_err(ToolBodyError::plain)?;
                    return Ok(serde_json::json!({
                        "kind": "background",
                        "jobId": id.as_str(),
                    }));
                }
                let run = subagents
                    .start(&provider, request)
                    .await
                    .map_err(|error| ToolBodyError::plain(error.message))?;
                settle_foreground_run(run)
                    .await
                    .map_err(ToolBodyError::plain)
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args: &serde_json::Value| {
            Some(ToolCallView::Generic {
                title: format!(
                    "Delegate: {}",
                    args.get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or("subagent")
                ),
                kind: Some(ToolCallKind::Other),
                raw_input: args.get("description").cloned(),
                content: None,
                locations: None,
            })
        })),
        present_result: None,
    };
    tools.register(ctx, definition).map(|_| ())
}

/// Job hooks that settle one background one-shot run (TS `settleStart`).
struct SettledRunHooks {
    run: parking_lot::Mutex<Option<tokio::task::JoinHandle<JobOutcome>>>,
    cancelled: Arc<parking_lot::Mutex<bool>>,
}

impl SettledRunHooks {
    fn new(
        start: std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Arc<dyn SubagentRun>, dsh_subagent::SubagentError>,
                    > + Send,
            >,
        >,
    ) -> Self {
        let cancelled = Arc::new(parking_lot::Mutex::new(false));
        let cancelled_for_task = cancelled.clone();
        let handle = tokio::spawn(async move {
            let outcome = match start.await {
                Ok(run) => settle_run(&run).await,
                Err(error) => {
                    if *cancelled_for_task.lock() {
                        JobOutcome {
                            status: JobOutcomeStatus::Killed,
                            detail: None,
                            output: None,
                        }
                    } else {
                        JobOutcome {
                            status: JobOutcomeStatus::Failed,
                            detail: Some(error.message),
                            output: None,
                        }
                    }
                }
            };
            outcome
        });
        Self {
            run: parking_lot::Mutex::new(Some(handle)),
            cancelled,
        }
    }
}

impl dsh_jobs::JobHooks for SettledRunHooks {
    fn cancel(&self, _reason: Option<String>) {
        *self.cancelled.lock() = true;
    }

    fn done(&self) -> cordis::BoxFuture<'static, JobOutcome> {
        let handle = self.run.lock().take();
        Box::pin(async move {
            match handle {
                Some(handle) => handle.await.unwrap_or(JobOutcome {
                    status: JobOutcomeStatus::Failed,
                    detail: Some("subagent task panicked".to_string()),
                    output: None,
                }),
                None => JobOutcome {
                    status: JobOutcomeStatus::Failed,
                    detail: Some("subagent task already settled".to_string()),
                    output: None,
                },
            }
        })
    }

    fn read_output(&self) -> Option<String> {
        None
    }
}

/// The Cordis plugin form.
pub struct ToolSubagentPlugin;

#[async_trait::async_trait]
impl Plugin for ToolSubagentPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        apply(ctx, &config).map_err(|error| PluginError::from(anyhow::anyhow!(error)))
    }
}
