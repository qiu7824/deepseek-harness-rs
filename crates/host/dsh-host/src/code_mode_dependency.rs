//! Validate the execution dependency before activating a code-mode preset.
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use std::sync::Arc;

pub struct CodeModePresentation {
    pub paths: Arc<crate::runtime_paths::RuntimePaths>,
    pub subprocess: Arc<dyn dsh_subprocess::SubprocessRuntime>,
}

#[async_trait::async_trait]
impl Plugin for CodeModePresentation {
    fn name(&self) -> Option<&'static str> {
        Some(dsh_agent_tool_presentation::NAME)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["tools"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let status = self.paths.node_status(self.subprocess.clone(), true).await;
        ensure_ready(&status).map_err(|error| PluginError::new(arc(error)))?;
        dsh_agent_tool_presentation::apply(
            ctx,
            dsh_agent_tool_presentation::Config {
                mode: dsh_tools::ToolPresentationMode::Code,
            },
        )
        .map_err(|error| PluginError::new(arc(error)))
    }
}

fn ensure_ready(status: &serde_json::Value) -> Result<(), String> {
    if status["available"] == true && status["status"] == "ready" {
        Ok(())
    } else {
        Err(format!(
            "PTC 代码模式需要可用的 Node.js 运行环境：{}。请在“目录与运行环境”中检查依赖，或选择标准模式。",
            status["error"].as_str().unwrap_or("运行环境尚未就绪")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn code_mode_requires_successful_capability_probe() {
        for status in ["missing", "incompatible", "timeout", "error"] {
            let error = ensure_ready(&json!({"available":false,"status":status})).unwrap_err();
            assert!(error.contains("Node.js"));
            assert!(error.contains("标准模式"));
        }
        assert!(ensure_ready(&json!({"available":true,"status":"ready"})).is_ok());
        assert!(ensure_ready(&json!({"available":true,"status":"error"})).is_err());
    }
}
