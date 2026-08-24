use std::sync::Arc;

use cordis::{ArcValue, InjectSpec, Plugin, PluginError, arc};
use dsh_commands::{CommandDefinition, CommandInvocation, CommandResult, CommandRuntime};
use dsh_compaction::{
    CompactionAbort, CompactionEngine, ManualCompactAgentContext, ManualCompactionErrorCode,
};

pub fn expected_failure_text(code: ManualCompactionErrorCode) -> &'static str {
    match code {
        ManualCompactionErrorCode::Busy => {
            "Compaction is unavailable because this process has an active compaction, or the agent is not idle."
        }
        ManualCompactionErrorCode::Cancelled => "Compaction cancelled.",
        ManualCompactionErrorCode::Changed => {
            "The history selected for compaction changed before it could be replaced. The conversation is unchanged; the attempt is recorded in the session log."
        }
        ManualCompactionErrorCode::Summary => {
            "Compaction could not produce a useful summary. The conversation is unchanged; the attempt is recorded in the session log."
        }
        ManualCompactionErrorCode::Commit => {
            "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying."
        }
        ManualCompactionErrorCode::Persistence => {
            "Compaction finished, but the session could not be saved."
        }
    }
}

pub fn apply(ctx: &cordis::Context) -> Result<cordis::Disposer, String> {
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "command-compact requires commands".to_string())?;
    let compaction = ctx
        .get_typed::<Arc<dyn CompactionEngine>>("compaction", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "command-compact requires compaction".to_string())?;
    commands.register(
        ctx,
        CommandDefinition {
            name: "compact".to_string(),
            description: "Compact older conversation history".to_string(),
            input: None,
            record_input: Some(false),
            handler: Arc::new(move |invocation: &CommandInvocation| {
                let engine = compaction.clone();
                let invocation = invocation.clone();
                Box::pin(async move {
                    if !invocation.raw_input.trim().is_empty() {
                        return Ok(CommandResult::Error {
                            text: "Usage: /compact (no arguments)".to_string(),
                        });
                    }
                    let signal: CompactionAbort = invocation.signal.clone();
                    let context = ManualCompactAgentContext {
                        session: invocation.agent.session().clone(),
                        provider: invocation.agent.options().provider.clone(),
                        model: invocation.agent.options().model.clone(),
                    };
                    match engine
                        .compact_now(&context, Some(&signal), Some(&invocation.command_id))
                        .await
                    {
                        Ok(None) => Ok(CommandResult::Success {
                            text: Some("No compactable history yet.".to_string()),
                            source_event_seq: None,
                        }),
                        Ok(Some(result)) => Ok(CommandResult::Success {
                            text: Some(format!(
                                "Compacted {} history items (~{} tokens).",
                                result.shadowed_seqs.len(),
                                result.shadowed_token_count
                            )),
                            source_event_seq: Some(result.summary_seq),
                        }),
                        Err(error) => Ok(CommandResult::Error {
                            text: expected_failure_text(error.code).to_string(),
                        }),
                    }
                })
            }),
        },
    )
}

pub struct CommandCompactPlugin;

#[async_trait::async_trait]
impl Plugin for CommandCompactPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("command-compact")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["commands", "compaction"])
    }

    async fn apply(&self, ctx: &cordis::Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx).map_err(|error| PluginError::new(arc(error)))?;
        let _ = ctx.effect(
            "command-compact lifecycle",
            Box::pin(async move { Some(disposer) }),
        );
        Ok(())
    }
}

pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(CommandCompactPlugin)
}
