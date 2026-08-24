//! Separately loadable `list_agents` plugin. Rust port of
//! `packages/subagent/tool-subagent-control/src/list-agents.ts`.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_agent::{AgentRegistry, AgentStatus};
use dsh_llm::ContentBlock;
use dsh_subagent::{SubagentIdentityProjection, SubagentListEntry, SubagentRuntime};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};

pub const NAME: &str = "tool-subagent-list-agents";
pub const INJECT: [&str; 3] = ["tools", "subagents", "agents"];

const DESCRIPTION: &str = "List your continuable background subagents by durable id and label. Use it to recall which ones you started, not to poll for completion — you are told when one finishes. Status comes from the live registry: running means the agent is working right now, idle means it is loaded but between turns (it may be waiting on agents it started), and ready means it exists only in storage — resumable, not terminal, and not a result waiting to be collected; a `send_message` starts a new turn on the same conversation, and a direct child remains a `send_message` candidate in every status. The snapshot is not a delivery promise — `send_message` performs the authoritative check and may still fail. Children that could not be read are reported as diagnostics instead of being silently dropped. Scope `descendants` walks the whole tree below you in stable pre-order, annotating each entry with its durable direct-parent session id and depth. You may use `send_message` only for depth-1 entries; deeper entries are candidates for `interrupt_agent` only.";

pub fn project(
    agents: &AgentRegistry,
    entry: SubagentListEntry,
    position: Option<(&dsh_session::SessionId, u64)>,
) -> Option<serde_json::Value> {
    let mut value = match entry {
        SubagentListEntry::Diagnostic { id, reason } => serde_json::json!({
            "kind": "diagnostic", "id": id.as_str(), "reason": reason
        }),
        SubagentListEntry::Child { id, identity, .. } => {
            let label = match identity {
                SubagentIdentityProjection::OneShot { .. } => return None,
                SubagentIdentityProjection::Continuable { label, .. } => label,
            };
            let status = match agents.get(&id).map(|agent| agent.status()) {
                None => "ready",
                Some(AgentStatus::Running) => "running",
                Some(AgentStatus::Idle) => "idle",
            };
            serde_json::json!({
                "kind": "child", "id": id.as_str(), "label": label, "status": status
            })
        }
    };
    if let Some((parent, depth)) = position {
        value["parent"] = serde_json::json!(parent.as_str());
        value["depth"] = serde_json::json!(depth);
    }
    Some(value)
}

pub fn apply(ctx: &Context) -> Result<cordis::Disposer, String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent-list-agents requires the tools service".to_string())?;
    let subagents = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent-list-agents requires the subagents service".to_string())?;
    let agents = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-subagent-list-agents requires the agents service".to_string())?;

    tools.register(ctx, ToolDefinition {
        name: "list_agents".to_string(),
        description: DESCRIPTION.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["children", "descendants"],
                    "description": "children (default) lists direct children only; descendants walks the complete tree below you."
                }
            },
            "required": []
        }),
        output: ToolOutputDefinition {
            schema: serde_json::json!({
                "type": "array",
                "items": { "oneOf": [
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "enum": ["child"] },
                            "id": { "type": "string" },
                            "label": { "type": "string" },
                            "status": { "type": "string", "enum": ["running", "idle", "ready"] },
                            "parent": { "type": "string" },
                            "depth": { "type": "number" }
                        },
                        "required": ["kind", "id", "label", "status"]
                    },
                    {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "enum": ["diagnostic"] },
                            "id": { "type": "string" },
                            "reason": { "type": "string", "enum": ["corrupt", "unsupported", "unavailable"] },
                            "parent": { "type": "string" },
                            "depth": { "type": "number" }
                        },
                        "required": ["kind", "id", "reason"]
                    }
                ] }
            }),
            render: Arc::new(|args, value| {
                let entries = value.as_array().cloned().unwrap_or_default();
                let descendants = args.get("scope").and_then(|v| v.as_str()) == Some("descendants");
                let text = if entries.is_empty() {
                    "(no subagents)".to_string()
                } else {
                    entries.iter().map(|entry| {
                        let at = if descendants {
                            format!(" parent={} depth={}", entry["parent"].as_str().unwrap_or_default(), entry["depth"])
                        } else { String::new() };
                        if entry["kind"] == "child" {
                            format!("{} [{}]{} — {}", entry["id"].as_str().unwrap_or_default(), entry["status"].as_str().unwrap_or_default(), at, entry["label"].as_str().unwrap_or_default())
                        } else {
                            format!("{} [diagnostic: {}]{}", entry["id"].as_str().unwrap_or_default(), entry["reason"].as_str().unwrap_or_default(), at)
                        }
                    }).collect::<Vec<_>>().join("\n")
                };
                Ok(vec![ContentBlock::Text { text }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let runtime = subagents.clone();
            let agents = agents.clone();
            let scope = args.get("scope").and_then(|value| value.as_str()).unwrap_or("children").to_string();
            let parent = exec.agent.clone();
            let signal = exec.signal.lock().clone();
            Box::pin(async move {
                let parent = parent.ok_or_else(|| ToolBodyError::plain(
                    "list_agents requires a calling agent (exec.agent was undefined)",
                ))?;
                let entries = match scope.as_str() {
                    "children" => runtime.list_children(parent.id(), Some(&signal)).await
                        .map_err(|error| ToolBodyError::plain(error.message))?
                        .into_iter().filter_map(|entry| project(&agents, entry, None)).collect(),
                    "descendants" => runtime.list_descendants(parent.id(), Some(&signal)).await
                        .map_err(|error| ToolBodyError::plain(error.message))?
                        .into_iter().filter_map(|entry| {
                            let parent_id = entry.parent_id.clone();
                            let depth = entry.depth;
                            project(&agents, entry.entry, Some((&parent_id, depth)))
                        }).collect(),
                    _ => return Err(ToolBodyError::plain("invalid list_agents scope")),
                };
                Ok(serde_json::Value::Array(entries))
            })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    })
}

pub struct ToolSubagentListAgentsPlugin;

#[async_trait::async_trait]
impl Plugin for ToolSubagentListAgentsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }
    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx).map_err(|error| PluginError::from(anyhow::anyhow!(error)))?;
        let _ = ctx.effect(NAME, Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
