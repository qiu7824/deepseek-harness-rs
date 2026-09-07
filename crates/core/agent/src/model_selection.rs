//! Agent-scoped model selection shared by runtime entry points. Rust port of
//! `packages/core/agent/src/model-selection.ts`.

use std::sync::Arc;

use cordis::{Context, Disposer, EventOptions, Listener, NextFn, arc, downcast};
use dsh_llm::{LlmCallConfig, ReasoningEffortId};
use dsh_system_prompt::SharedAssembly;
use parking_lot::Mutex;

/// A switch is conversation context, not a system-prompt mutation: keeping
/// it in the durable message history preserves the stable cache prefix.
pub fn model_switch_notice(
    previous: Option<&LlmCallConfig>,
    selected: &LlmCallConfig,
) -> Option<dsh_llm::UserMessage> {
    let previous = previous?;
    if previous.provider == selected.provider && previous.model == selected.model {
        return None;
    }
    Some(dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: format!(
                "The active model changed from {}/{} to {}/{}. Continue the existing task with its current instructions and progress.",
                previous.provider, previous.model, selected.provider, selected.model
            ),
        }],
        dsh_llm::MessageSource::Plugin {
            plugin: "model-selection".into(),
            form: None,
            sections: None,
            summary: Some("Active model changed".into()),
            compaction_id: None,
            source_command_id: None,
        },
    ))
}

/// Complete provider, model, and optional reasoning effort selected for one
/// live Agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    pub execution_mode: dsh_llm::ExecutionMode,

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
            .map(|mut selected| {
                if selected.provider == "openai-codex"
                    && selected
                        .reasoning_effort
                        .as_ref()
                        .is_some_and(|e| e.as_str() == "ultra")
                {
                    selected.execution_mode = dsh_llm::ExecutionMode::Ultra;
                }
                selected
            })
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
    let model_runtime = agent_ctx
        .get_typed::<Arc<dsh_llm::LlmRuntime>>("llm", false)
        .map(|s| s.as_ref().clone());
    let assembly_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let selection = Arc::clone(&selection_for_assembly);
        let model_runtime = model_runtime.clone();
        Box::pin(async move {
            let next = downcast::<NextFn>(&args[2]).expect("assemble next continuation");
            let mut selected = selection.lock().resolved_current();
            if let (Some(runtime), Some(value)) = (&model_runtime, &mut selected) {
                if value.execution_mode == dsh_llm::ExecutionMode::Ultra
                    || value
                        .reasoning_effort
                        .as_ref()
                        .is_some_and(|e| e.as_str() == "ultra")
                {
                    if let Ok(config) = runtime
                        .resolve_call_config(
                            &LlmCallConfig {
                                provider: value.provider.clone(),
                                model: value.model.clone(),
                                reasoning_effort: value.reasoning_effort.clone(),
                                execution_mode: value.execution_mode,
                                ..Default::default()
                            },
                            None,
                        )
                        .await
                    {
                        value.execution_mode = config.execution_mode;
                        value.reasoning_effort = config.reasoning_effort;
                    }
                }
            }
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
            if selected.execution_mode == dsh_llm::ExecutionMode::Ultra {
                merged.sections.push(dsh_system_prompt::AssembledSection {
                    name: "execution:ultra".into(),
                    text: "For the root task, use proactive delegation when independent subtasks improve speed or quality. Give each child a bounded task and expected result; continue useful work yourself. Use at most three active children. If you are a delegated child, complete your assigned work directly and never delegate further. For a simple task, work directly. Collect required results, resolve failures, verify the combined work, and stop all remaining children before finishing. Existing user instructions and permissions remain in force.".into(),
                });
            }
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
                execution_mode: selected.execution_mode,
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
