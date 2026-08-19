//! First-human-message model provider for `ctx.sessionTitle`. Rust port of
//! `packages/session/session-title-first-prompt-llm/src/index.ts`.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc, downcast};
use dsh_session_title_llm::{SessionTitleLlmConfig, register_session_title_llm_provider};

/// Cordis plugin name (TS `name`).
pub const NAME: &str = "session-title-first-prompt-llm";

/// Required services (TS `inject`).
pub const INJECT: [&str; 3] = ["sessionTitle", "llm", "sessions"];

/// Required LLM policy; this plugin adds no defaults (TS `Config`).
pub type Config = SessionTitleLlmConfig;

/// Register the first-prompt model provider (TS `apply`).
pub fn apply(ctx: &Context, config: Config) -> Result<(), String> {
    register_session_title_llm_provider(
        ctx,
        config,
        NAME,
        dsh_session_title::SessionTitleAutomaticMode::FirstPrompt,
        Arc::new(|messages| match messages.into_iter().next() {
            Some(first) => Ok(vec![first]),
            None => Err("first-prompt title provider requires one human message".to_string()),
        }),
    )
    .map(|_| ())
}

/// The Cordis plugin form.
pub struct SessionTitleFirstPromptLlmPlugin;

#[async_trait::async_trait]
impl Plugin for SessionTitleFirstPromptLlmPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = downcast::<Config>(&config)
            .cloned()
            .or_else(|| {
                serde_json::from_value(downcast::<serde_json::Value>(&config)?.clone()).ok()
            })
            .ok_or_else(|| {
                PluginError::new(arc(
                    "session-title-first-prompt-llm: configuration is required".to_string(),
                ))
            })?;
        apply(ctx, config).map_err(|error| PluginError::new(arc(error)))
    }
}
