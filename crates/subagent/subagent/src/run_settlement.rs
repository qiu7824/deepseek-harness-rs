//! Settlement of one ONE-SHOT subagent run into a background-Task outcome.
//! Rust port of `packages/subagent/subagent/src/run-settlement.ts`.

use std::sync::Arc;

use dsh_jobs::{JobOutcome, JobOutcomeStatus};
use dsh_llm::ContentBlock;

use crate::types::{SubagentResult, SubagentRun, SubagentStopReason};

/// Flatten a child's final output blocks to the task's final text.
fn final_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Map a child result to the task outcome: completed carries final text,
/// aborted is killed, and every other reason is failed without partial
/// output.
fn run_outcome(result: &SubagentResult) -> JobOutcome {
    match result.stop_reason {
        SubagentStopReason::Completed => JobOutcome {
            status: JobOutcomeStatus::Completed,
            detail: None,
            output: Some(final_text(&result.output)),
        },
        SubagentStopReason::Aborted => JobOutcome {
            status: JobOutcomeStatus::Killed,
            detail: None,
            output: None,
        },
        reason => JobOutcome {
            status: JobOutcomeStatus::Failed,
            detail: Some(reason.as_str().to_string()),
            output: None,
        },
    }
}

/// Await the child result, dispose the run, then return its task outcome.
pub async fn settle_run(run: &Arc<dyn SubagentRun>) -> JobOutcome {
    let outcome = match run.result().await {
        Ok(result) => run_outcome(&result),
        Err(error) => JobOutcome {
            status: JobOutcomeStatus::Failed,
            detail: Some(error),
            output: None,
        },
    };
    if let Err(error) = run.dispose().await {
        let prefix = match &outcome.detail {
            None => String::new(),
            Some(detail) => format!("{detail}; "),
        };
        return JobOutcome {
            status: JobOutcomeStatus::Failed,
            detail: Some(format!("{prefix}dispose failed: {error}")),
            output: None,
        };
    }
    outcome
}
