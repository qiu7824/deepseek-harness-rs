//! Session feedback event plus the human-facing `/feedback` producer.
//! Recording appends one authoritative log-only event and does not start
//! model work. The append is eager but unflushed, so acknowledgement reports
//! that the entry is logged, not that it reached disk.
//! Rust port of `packages/feedback/command-feedback/src/index.ts`.

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_anonymous_user_id::{AnonymousUserIdOptions, get_or_create_anonymous_user_id};
use dsh_commands::{
    CommandDefinition, CommandInputDescriptor, CommandInvocation, CommandResult, CommandRuntime,
};
use dsh_session::Session;
use dsh_session_telemetry::{SessionTelemetryBackend, SessionTelemetrySharingStatus};

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "command-feedback";

/// The command registry service this plugin registers into.
pub const INJECT: [&str; 1] = ["commands"];

const USAGE: &str = "Usage: /feedback <text>";

/// The acknowledgement's sharing sentence for a disclosed policy (TS
/// `sharingSentence`).
pub fn sharing_sentence(sharing: SessionTelemetrySharingStatus) -> &'static str {
    match sharing {
        SessionTelemetrySharingStatus::Full => "Session sharing is enabled.",
        SessionTelemetrySharingStatus::FeedbackOnly => {
            "Session sharing is feedback-gated; recording feedback releases the session prefix for sharing."
        }
        SessionTelemetrySharingStatus::Disabled => "Session sharing is disabled.",
    }
}

/// The sharing disclosure appended to the acknowledgement (TS
/// `sharingDisclosure`).
pub fn sharing_disclosure(telemetry: Option<&dyn SessionTelemetryBackend>) -> &'static str {
    match telemetry {
        None => "Session sharing is not configured.",
        Some(telemetry) => sharing_sentence(telemetry.sharing()),
    }
}

/// Record feedback independently of any UI trigger (TS `recordFeedback`).
pub fn record_feedback(session: &Session, text: &str) -> Result<dsh_session::SessionEvent, String> {
    let normalized = text.trim();
    if normalized.is_empty() {
        return Err("feedback text must not be empty".to_string());
    }
    session.append(
        "feedback/record",
        serde_json::json!({ "text": normalized }),
        None,
    )
}

/// Validate, record, and acknowledge one feedback entry (TS
/// `executeFeedbackCommand`). An empty input returns the usage error result
/// and records nothing.
pub fn execute_feedback_command(
    invocation: &CommandInvocation,
    ctx: &Context,
) -> Result<CommandResult, String> {
    if invocation.raw_input.trim().is_empty() {
        return Ok(CommandResult::Error {
            text: format!("Feedback text is required. {USAGE}"),
        });
    }
    record_feedback(invocation.agent.session(), &invocation.raw_input)?;
    let telemetry = ctx
        .get_typed::<Arc<dyn SessionTelemetryBackend>>("sessionTelemetry", false)
        .map(|slot| slot.as_ref().clone());
    let disclosure = match &telemetry {
        Some(telemetry) => sharing_sentence(telemetry.sharing()),
        None => "Session sharing is not configured.",
    };
    let anonymous_id = get_or_create_anonymous_user_id(AnonymousUserIdOptions::default());
    Ok(CommandResult::Success {
        text: Some(format!(
            "Feedback recorded for session {}\nAnonymous user: {anonymous_id}. {disclosure}",
            invocation.agent.session().id(),
        )),
        source_event_seq: None,
    })
}

/// Register the global `/feedback` command for every composed command
/// adapter (TS `apply`).
pub fn apply(ctx: &Context) -> Result<cordis::Disposer, String> {
    let ctx = ctx.clone();
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "command-feedback requires the commands service".to_string())?;
    let handler_ctx = ctx.clone();
    commands.register(
        &ctx,
        CommandDefinition {
            name: "feedback".to_string(),
            description: "记录对此会话的反馈".to_string(),
            input: Some(CommandInputDescriptor {
                hint: "<text>".to_string(),
            }),
            record_input: Some(false),
            handler: Arc::new(move |invocation| {
                let ctx = handler_ctx.clone();
                let invocation = invocation.clone();
                Box::pin(async move { execute_feedback_command(&invocation, &ctx) })
            }),
        },
    )
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `apply`).
pub struct CommandFeedbackPlugin;

#[async_trait::async_trait]
impl Plugin for CommandFeedbackPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("command-feedback", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}

// The invocation's abort signal is read by the command runtime; the producer
// itself never inspects it (documented for the TS parity note).
pub use dsh_commands::CommandAbort as CommandAbortType;
