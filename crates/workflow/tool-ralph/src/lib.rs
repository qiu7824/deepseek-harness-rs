//! Model-facing foreground Ralph loop over the workflow seam.

pub mod invariant;

use std::sync::Arc;

use cordis::{Context, Disposer};
use dsh_llm::ContentBlock;
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use dsh_workflow::{
    WorkflowEngine, WorkflowMeta, WorkflowPhase, WorkflowStartRequest, WorkflowStopReason,
};
use serde_json::{Value, json};

pub const NAME: &str = "ralph";
pub const DEFAULT_SUBAGENT_PROVIDER: &str = "spawn";
pub const DEFAULT_MAX_ROUNDS: u64 = 256;
pub const DEFAULT_MAX_HANDOFF_CHARS: usize = 16_384;
pub const DEFAULT_MAX_RESULT_CHARS: usize = 16_384;

const RALPH_SCRIPT: &str = r#"
const reportSchema = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['continue', 'complete', 'blocked'] },
    summary: { type: 'string' },
    evidence: { type: 'array', items: { type: 'string' } },
    nextSteps: { type: 'array', items: { type: 'string' } },
    blocker: { type: 'string' },
  },
  required: ['status', 'summary', 'evidence', 'nextSteps', 'blocker'],
  additionalProperties: false,
};
let previous;
phase('Fresh-agent rounds');
for (let round = 1; round <= args.maxRounds; round += 1) {
  const report = await agent({
    prompt: 'Immutable objective:\n' + args.objective + '\n\nRalph round ' + round + ' of ' + args.maxRounds + '.\nPrevious handoff:\n' + JSON.stringify(previous ?? null),
    label: 'Ralph round ' + round,
    phase: 'Fresh-agent rounds',
    schema: reportSchema,
  });
  if (report === null) return { status: 'round-failed', roundsStarted: round, lastReport: previous ?? null };
  if (report.status === 'complete') return { status: 'complete', roundsStarted: round, report };
  if (report.status === 'blocked') return { status: 'blocked', roundsStarted: round, report };
  previous = report;
}
return { status: 'budget-limited', roundsStarted: args.maxRounds, report: previous };
"#;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub subagent_provider: Option<String>,
    pub max_rounds: Option<u64>,
    pub max_handoff_chars: Option<usize>,
    pub max_result_chars: Option<usize>,
}

struct ResolvedConfig {
    provider: String,
    max_rounds: u64,
    max_handoff_chars: usize,
    max_result_chars: usize,
}

pub fn parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "objective": { "type": "string" },
            "maxRounds": { "type": "number" }
        },
        "required": ["objective"]
    })
}

pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "runId": { "type": "string" },
            "agentsStarted": { "type": "integer" },
            "result": {}
        },
        "required": ["runId", "agentsStarted", "result"]
    })
}

pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    let resolved = resolve_config(config)?;
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-ralph requires the tools service".to_string())?;
    let engine = ctx
        .get_typed::<Arc<dyn WorkflowEngine>>("workflowEngine", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-ralph requires the workflowEngine service".to_string())?;
    let render_limit = resolved.max_result_chars;
    tools.register(ctx, ToolDefinition {
        name: NAME.to_string(),
        description: "Run a foreground fresh-agent Ralph loop toward one immutable objective.".to_string(),
        parameters: parameters_schema(),
        output: ToolOutputDefinition {
            schema: output_schema(),
            render: Arc::new(move |_args, value| {
                let result = &value["result"];
                let rounds = result["roundsStarted"].as_u64().unwrap_or_default();
                let status = result["status"].as_str().unwrap_or("unknown");
                let label = match status {
                    "complete" => "completion",
                    "blocked" => "a blocker",
                    "budget-limited" => "work remaining at the round limit",
                    other => other,
                };
                let report = result
                    .get("report")
                    .and_then(|report| serde_json::to_string_pretty(report).ok())
                    .unwrap_or_else(|| "(no final report)".to_string());
                Ok(vec![ContentBlock::Text {
                    text: bound_result(
                        format!(
                            "Ralph worker reported {label} after {rounds} round{}.\nFinal report:\n{report}",
                            if rounds == 1 { "" } else { "s" }
                        ),
                        render_limit,
                    ),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, run| {
            let engine = engine.clone();
            let args = args.clone();
            let parent = run.execution.agent.clone();
            let signal = run.execution.signal.lock().clone();
            let provider = resolved.provider.clone();
            let ceiling = resolved.max_rounds;
            let max_handoff_chars = resolved.max_handoff_chars;
            Box::pin(async move {
                let parent = parent.ok_or_else(|| ToolBodyError::plain("Ralph requires a calling agent"))?;
                let objective = args.get("objective").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if objective.is_empty() {
                    return Err(ToolBodyError::plain("Ralph objective must be non-empty"));
                }
                let rounds = args.get("maxRounds").and_then(Value::as_u64).unwrap_or(ceiling);
                if rounds == 0 || rounds > ceiling {
                    return Err(ToolBodyError::plain(format!("Ralph maxRounds must be between 1 and {ceiling}")));
                }
                let workflow = engine.start(WorkflowStartRequest {
                    script: RALPH_SCRIPT.to_string(),
                    meta: WorkflowMeta {
                        name: "ralph-loop".to_string(),
                        description: "Iterate toward one objective with a fresh child and bounded structured handoff per round.".to_string(),
                        when_to_use: None,
                        phases: vec![WorkflowPhase {
                            title: "Fresh-agent rounds".to_string(),
                            detail: Some("One clean child context per Ralph round.".to_string()),
                            provider: None,
                            model: None,
                        }],
                    },
                    args: Some(json!({
                        "objective": objective,
                        "maxRounds": rounds,
                        "maxHandoffChars": max_handoff_chars,
                    })),
                    subagent_provider: Some(provider),
                    max_total_agents: Some(rounds),
                    parent,
                    signal: Some(signal),
                }).map_err(|error| ToolBodyError::coded(error.to_string(), "WorkflowError", error.code()))?;
                let result = workflow.result().await;
                let run_id = workflow.id().as_str().to_string();
                workflow.dispose().await;
                if result.stop_reason != WorkflowStopReason::Completed {
                    return Err(ToolBodyError::coded(
                        result.error.unwrap_or_else(|| "Ralph workflow did not complete".to_string()),
                        "RalphWorkflowError",
                        "RALPH_WORKFLOW_FAILED",
                    ));
                }
                Ok(json!({
                    "runId": run_id,
                    "agentsStarted": result.agents_started,
                    "result": result.value,
                }))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    })
}

fn bound_result(text: String, max_chars: usize) -> String {
    const NOTICE: &str = "\n… [truncated]";
    if text.chars().count() <= max_chars {
        return text;
    }
    if max_chars <= NOTICE.chars().count() {
        return NOTICE.chars().take(max_chars).collect();
    }
    let keep = max_chars - NOTICE.chars().count();
    format!("{}{}", text.chars().take(keep).collect::<String>(), NOTICE)
}

fn resolve_config(config: &Config) -> Result<ResolvedConfig, String> {
    let provider = config
        .subagent_provider
        .clone()
        .unwrap_or_else(|| DEFAULT_SUBAGENT_PROVIDER.to_string());
    let max_rounds = config.max_rounds.unwrap_or(DEFAULT_MAX_ROUNDS);
    let max_handoff_chars = config
        .max_handoff_chars
        .unwrap_or(DEFAULT_MAX_HANDOFF_CHARS);
    let max_result_chars = config.max_result_chars.unwrap_or(DEFAULT_MAX_RESULT_CHARS);
    if provider.trim().is_empty() || provider != provider.trim() {
        return Err("subagent_provider must be a non-empty normalized string".to_string());
    }
    if max_rounds == 0 || max_handoff_chars == 0 || max_result_chars == 0 {
        return Err("Ralph limits must be positive".to_string());
    }
    Ok(ResolvedConfig {
        provider,
        max_rounds,
        max_handoff_chars,
        max_result_chars,
    })
}
