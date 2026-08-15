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
#[derive(Debug, Clone, Default)]
pub struct ModelSelectionRef {
    /// Model selected for the next step that enters prompt assembly.
    pub current: Option<ModelSelection>,
    /// Selection captured when the current step entered prompt assembly.
    pub assembled: Option<ModelSelection>,
}

/// Couple one mutable selection to Agent-scoped prompt assembly and request
/// routing (TS `installModelSelection`). The selection ref is shared; the
/// agent-loop mutates `current` between steps.
pub fn install_model_selection(
    agent_ctx: &Context,
    selection: Arc<Mutex<ModelSelectionRef>>,
) -> Disposer {
    let selection_for_assembly = Arc::clone(&selection);
    let assembly_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let selection = Arc::clone(&selection_for_assembly);
        Box::pin(async move {
            let next = downcast::<NextFn>(&args[2]).expect("assemble next continuation");
            let selected = selection.lock().current.clone();
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
            let next = downcast::<NextFn>(&args[2]).expect("request next continuation");
            let value = next.call().await;
            let resolved = downcast::<LlmCallConfig>(&value).cloned().unwrap_or_default();
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

    let assembly_disposer = futures::executor::block_on(agent_ctx.on(
        "system-prompt/assemble",
        assembly_listener,
        EventOptions::default(),
    ));
    let request_disposer = futures::executor::block_on(agent_ctx.on(
        "agent/request",
        request_listener,
        EventOptions::default(),
    ));
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
