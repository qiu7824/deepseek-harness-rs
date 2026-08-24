//! Globally named continuable-subagent control tools. Rust port of
//! `packages/subagent/tool-subagent-control/src/index.ts`.

pub mod list_agents;

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_llm::{ContentBlock, ContextForm, MessageSource};
use dsh_session::session_id;
use dsh_subagent::{SubagentFollowupOptions, SubagentInterruptAuthority, SubagentRuntime};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};

pub const NAME: &str = "tool-subagent-control";
pub const INJECT: [&str; 2] = ["tools", "subagents"];

const SEND_DESCRIPTION: &str = "Send a message to a background subagent by its subagent id, continuing the same conversation. It becomes the subagent's next turn: if it is still working, the message waits until its current turn finishes, so it cannot redirect work already underway. This call returns no answer from the subagent — only confirmation that the message was delivered — so use it to give it more work. A failure means the message was NOT delivered.";
const INTERRUPT_DESCRIPTION: &str = "Request cancellation of a background agent's current turn by its agent id. The target may be your direct child or a deeper agent created under you. Only the current turn stops: messages already queued for the agent stay parked until a later send_message, agents it started keep running, and the agent itself stays available for follow-ups. This call returns as soon as the stop request is accepted, so the target may keep running briefly; interrupting an agent that already finished is an accepted no-op.";

pub fn apply(ctx: &Context) -> Result<Vec<cordis::Disposer>, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent-control requires the tools service".to_string())?;
    let subagents = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent-control requires the subagents service".to_string())?;

    let send_runtime = subagents.clone();
    let send = tools.register(ctx, ToolDefinition {
        name: "send_message".to_string(),
        description: SEND_DESCRIPTION.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "subagent_id": { "type": "string", "description": "The subagent id returned when the background subagent was started." },
                "message": { "type": "string", "description": "The message to deliver to the subagent." }
            },
            "required": ["subagent_id", "message"]
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "messageId": { "type": "string" } },
                "required": ["messageId"]
            }),
            render: Arc::new(|args, _value| Ok(vec![ContentBlock::Text {
                text: format!("message queued as the next turn for subagent {}", args["subagent_id"].as_str().unwrap_or_default()),
            }])),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let runtime = send_runtime.clone();
            let args = args.clone();
            let parent = exec.agent.clone();
            let signal = exec.signal.lock().clone();
            Box::pin(async move {
                let parent = parent.ok_or_else(|| ToolBodyError::plain(
                    "send_message requires a calling agent (exec.agent was undefined)",
                ))?;
                let child_id = session_id(args["subagent_id"].as_str().unwrap_or_default());
                let message = vec![ContentBlock::Text {
                    text: args["message"].as_str().unwrap_or_default().to_string(),
                }];
                let message_id = runtime.followup(
                    parent.clone(),
                    &child_id,
                    &message,
                    SubagentFollowupOptions {
                        source: MessageSource::Coordinator {
                            form: ContextForm::Relay,
                            sender_session_id: parent.id().as_str().to_string(),
                        },
                        signal,
                    },
                ).await.map_err(|error| ToolBodyError::plain(error.message))?;
                Ok(serde_json::json!({ "messageId": message_id.as_str() }))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    })?;

    let interrupt = tools.register(ctx, ToolDefinition {
        name: "interrupt_agent".to_string(),
        description: INTERRUPT_DESCRIPTION.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "agent_id": { "type": "string", "description": "The agent id of the running agent to interrupt." }
            },
            "required": ["agent_id"]
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "accepted": { "type": "boolean" } },
                "required": ["accepted"]
            }),
            render: Arc::new(|args, _value| Ok(vec![ContentBlock::Text {
                text: format!("interrupt requested for agent {}", args["agent_id"].as_str().unwrap_or_default()),
            }])),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let runtime = subagents.clone();
            let args = args.clone();
            let caller = exec.agent.clone();
            Box::pin(async move {
                let caller = caller.ok_or_else(|| ToolBodyError::plain(
                    "interrupt_agent requires a calling agent (exec.agent was undefined)",
                ))?;
                runtime.interrupt(
                    &session_id(args["agent_id"].as_str().unwrap_or_default()),
                    &SubagentInterruptAuthority::Ancestor { agent: caller },
                ).map_err(|error| ToolBodyError::plain(error.message))?;
                Ok(serde_json::json!({ "accepted": true }))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    })?;
    Ok(vec![send, interrupt])
}

pub struct ToolSubagentControlPlugin;

#[async_trait::async_trait]
impl Plugin for ToolSubagentControlPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }
    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposers = apply(ctx).map_err(|error| PluginError::from(anyhow::anyhow!(error)))?;
        let _ = ctx.effect(
            NAME,
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let disposers = disposers.clone();
                    Box::pin(async move {
                        for dispose in disposers {
                            dispose().await;
                        }
                    })
                }))
            }),
        );
        Ok(())
    }
}
