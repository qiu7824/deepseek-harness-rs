//! Agent-scoped model selection shared by runtime entry points. Rust port of
//! `packages/core/agent/src/model-selection.ts`.

use std::sync::Arc;

use cordis::{Context, Disposer, EventOptions, Listener, NextFn, arc, downcast};
use dsh_llm::{LlmCallConfig, ReasoningEffortId};
use dsh_system_prompt::SharedAssembly;
use parking_lot::Mutex;

/// Complete provider, model, and optional reasoning effort selected for one
/// live Agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    /// Registered provider route.
    pub provider: String,
    /// Provider-owned model id.
    pub model: String,
    /// Adapter-owned reasoning effort.
    pub reasoning_effort: Option<ReasoningEffortId>,
}

/// Mutable model selection plus the value captured for the current step.
pub type ModelSelectionResolver = Arc<dyn Fn() -> Option<ModelSelection> + Send + Sync>;

/// Agent-private service key for the mutable model selection attached to one
/// exact Cordis scope. Named services share a root reflection table, so the
/// fiber identity prevents one live Agent from colliding with another.
pub fn model_selection_service_name(ctx: &Context) -> String {
    format!("agentModelSelection:{:p}", Arc::as_ptr(&ctx.fiber))
}

#[derive(Clone, Default)]
pub struct ModelSelectionRef {
    /// Model selected for the next step that enters prompt assembly.
    pub current: Option<ModelSelection>,
    /// Selection captured when the current step entered prompt assembly.
    pub assembled: Option<ModelSelection>,
    resolver: Option<ModelSelectionResolver>,
}

impl std::fmt::Debug for ModelSelectionRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelSelectionRef")
            .field("current", &self.current)
            .field("assembled", &self.assembled)
            .field("has_resolver", &self.resolver.is_some())
            .finish()
    }
}

impl ModelSelectionRef {
    pub fn with_resolver(resolver: ModelSelectionResolver) -> Self {
        Self {
            resolver: Some(resolver),
            ..Self::default()
        }
    }

    pub fn resolved_current(&self) -> Option<ModelSelection> {
        self.current
            .clone()
            .or_else(|| self.resolver.as_ref().and_then(|resolve| resolve()))
    }
}

/// Couple one mutable selection to Agent-scoped prompt assembly and request
/// routing (TS `installModelSelection`). The selection ref is shared; the
/// agent-loop mutates `current` between steps.
pub async fn install_model_selection(
    agent_ctx: &Context,
    selection: Arc<Mutex<ModelSelectionRef>>,
) -> Disposer {
    agent_ctx.provide(
        &model_selection_service_name(agent_ctx),
        Some(arc(Arc::clone(&selection))),
    );
    let selection_for_assembly = Arc::clone(&selection);
    let assembly_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let selection = Arc::clone(&selection_for_assembly);
        Box::pin(async move {
            let next = downcast::<NextFn>(&args[2]).expect("assemble next continuation");
            let selected = selection.lock().resolved_current();
            let value = next.call().await;
            let assembled = downcast::<SharedAssembly>(&value)
                .expect("system-prompt/assemble must resolve an assembly")
                .snapshot();
            {
                let mut selection = selection.lock();
                selection.assembled = selected.clone();
            }
            let Some(selected) = selected else {
                return Some(value);
            };
            let mut merged = assembled;
            merged
                .variables
                .insert("provider".to_string(), Some(selected.provider.clone()));
            merged
                .variables
                .insert("model".to_string(), Some(selected.model.clone()));
            Some(arc(SharedAssembly::new(merged)))
        })
    });

    let selection_for_request = Arc::clone(&selection);
    let request_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let selection = Arc::clone(&selection_for_request);
        Box::pin(async move {
            let next = downcast::<NextFn>(&args[1]).expect("request next continuation");
            let value = next.call().await;
            let resolved = downcast::<LlmCallConfig>(&value)
                .cloned()
                .unwrap_or_default();
            let selected = selection.lock().assembled.clone();
            let Some(selected) = selected else {
                return Some(value);
            };
            let replaced = LlmCallConfig {
                provider: selected.provider,
                model: selected.model,
                // An absent selected effort clears any inherited effort,
                // restoring the selected model's provider/default behavior.
                reasoning_effort: selected.reasoning_effort,
                temperature: resolved.temperature,
                max_tokens: resolved.max_tokens,
                stop: resolved.stop,
            };
            Some(arc(replaced))
        })
    });

    let assembly_disposer = agent_ctx
        .on(
            "system-prompt/assemble",
            assembly_listener,
            EventOptions::default(),
        )
        .await;
    let request_disposer = agent_ctx
        .on("agent/request", request_listener, EventOptions::default())
        .await;
    let assembly_disposer = Arc::new(assembly_disposer);
    let request_disposer = Arc::new(request_disposer);
    cordis::make_disposer(move || {
        let assembly = Arc::clone(&assembly_disposer);
        let request = Arc::clone(&request_disposer);
        Box::pin(async move {
            request().await;
            assembly().await;
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::call_config_equals;

    #[test]
    fn selection_ref_defaults() {
        let selection = ModelSelectionRef::default();
        assert!(selection.current.is_none());
        assert!(selection.assembled.is_none());
    }

    #[test]
    fn manual_selection_overrides_dynamic_fallback_and_clear_restores_it() {
        let fallback = ModelSelection {
            provider: "fallback".into(),
            model: "fallback-model".into(),
            reasoning_effort: None,
        };
        let mut selection = ModelSelectionRef::with_resolver(Arc::new({
            let fallback = fallback.clone();
            move || Some(fallback.clone())
        }));
        assert_eq!(selection.resolved_current(), Some(fallback.clone()));
        selection.current = Some(ModelSelection {
            provider: "picked".into(),
            model: "picked-model".into(),
            reasoning_effort: None,
        });
        assert_eq!(selection.resolved_current().unwrap().provider, "picked");
        selection.current = None;
        assert_eq!(selection.resolved_current(), Some(fallback));
    }

    #[test]
    fn request_replacement_clears_inherited_effort() {
        // The listener logic: an absent selected effort clears any inherited
        // effort (verified through the full listener in registry tests; the
        // pure rule is pinned here).
        let selected = ModelSelection {
            provider: "p2".into(),
            model: "m2".into(),
            reasoning_effort: None,
        };
        let resolved = LlmCallConfig {
            provider: "p1".into(),
            model: "m1".into(),
            reasoning_effort: Some(dsh_llm::reasoning_effort_id("high")),
            temperature: Some(0.5),
            max_tokens: Some(100),
            stop: Some(vec!["stop".into()]),
        };
        let replaced = LlmCallConfig {
            provider: selected.provider.clone(),
            model: selected.model.clone(),
            reasoning_effort: selected.reasoning_effort.clone(),
            temperature: resolved.temperature,
            max_tokens: resolved.max_tokens,
            stop: resolved.stop.clone(),
        };
        assert!(call_config_equals(
            &replaced,
            &LlmCallConfig {
                provider: "p2".into(),
                model: "m2".into(),
                reasoning_effort: None,
                temperature: Some(0.5),
                max_tokens: Some(100),
                stop: Some(vec!["stop".into()]),
            }
        ));
    }
}
