use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_tools::{
    PreToolDecision, ToolBodyError, ToolDefinition, ToolExecution, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};

#[derive(Debug, Clone)]
pub struct Config {
    pub command: String,
    pub timeout_ms: u64,
}

const READ_ONLY_ACTIONS: &[&str] = &["capture", "list_apps", "list_windows", "cua_browser_state"];

pub fn action_requires_approval(action: &str) -> bool {
    !READ_ONLY_ACTIONS.contains(&action)
}

pub fn install(ctx: &Context, config: Config) -> Result<(), String> {
    if config.command.trim().is_empty() {
        return Err("computer-use command must be non-empty".to_string());
    }
    if config.timeout_ms < 1_000 || config.timeout_ms > 300_000 {
        return Err("computer-use timeout must be between 1000 and 300000 ms".to_string());
    }
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "computer-use command requires the tools service".to_string())?;

    let listener: Arc<Listener> = Arc::new(|_ctx, args| {
        let execution = args
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|slot| slot.as_ref().clone());
        let next = args.last().and_then(|value| downcast_arc::<NextFn>(value));
        Box::pin(async move {
            if let Some(execution) = execution
                && execution.name == "computer_use"
                && action_requires_approval(
                    execution
                        .arguments
                        .get("action")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                )
            {
                return Some(arc(PreToolDecision::Ask {
                    reason: Some("Computer Use 将操作桌面或浏览器，需要用户确认".to_string()),
                }));
            }
            let Some(next) = next else {
                return Some(arc(PreToolDecision::Allow));
            };
            Some(next.call().await)
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    ));

    let command = config.command;
    let timeout_ms = config.timeout_ms;
    tools.register(
        ctx,
        ToolDefinition {
            name: "computer_use".to_string(),
            description: "Operate a desktop or browser through a configured external command adapter. Capture state before interaction and verify after mutations.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "additionalProperties": true,
                "properties": { "action": { "type": "string" } },
                "required": ["action"]
            }),
            output: ToolOutputDefinition {
                schema: serde_json::json!({}),
                render: Arc::new(|_args, value| Ok(vec![dsh_llm::ContentBlock::Text {
                    text: serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()),
                }])),
                presentation_meta: None,
            },
            timeout_ms: Some(timeout_ms),
            is_concurrency_safe: Some(Arc::new(|args| {
                !action_requires_approval(
                    args.get("action").and_then(|value| value.as_str()).unwrap_or_default(),
                )
            })),
            execute: Arc::new(move |args, run: &ToolRunContext| {
                let command = command.clone();
                let payload = args.clone();
                let action = payload
                    .get("action")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let signal = run.signal.lock().clone();
                Box::pin(async move {
                    let serialized = serde_json::to_string(&payload)
                        .map_err(|error| ToolBodyError::plain(error.to_string()))?;
                    let output = dsh_native_command::run_native_command(
                        &command,
                        &[action, serialized],
                        Some(signal),
                    )
                    .await
                    .map_err(|error| {
                        ToolBodyError::plain(format!(
                            "computer-use adapter failed: {}{}",
                            error,
                            if error.stderr.trim().is_empty() {
                                String::new()
                            } else {
                                format!(": {}", error.stderr.trim())
                            }
                        ))
                    })?;
                    serde_json::from_str(output.stdout.trim()).map_err(|error| {
                        ToolBodyError::plain(format!(
                            "computer-use adapter returned invalid JSON: {error}"
                        ))
                    })
                })
            }),
            finalize_content: None,
            present_call: None,
            present_result: None,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::action_requires_approval;

    #[test]
    fn mutations_require_approval_but_observation_does_not() {
        for action in ["click", "type", "key", "cua_browser_navigate"] {
            assert!(action_requires_approval(action), "{action}");
        }
        for action in ["capture", "list_apps", "list_windows", "cua_browser_state"] {
            assert!(!action_requires_approval(action), "{action}");
        }
        for action in ["tap", "write_file", "close_app", ""] {
            assert!(
                action_requires_approval(action),
                "unknown action must fail closed: {action}"
            );
        }
    }
}
