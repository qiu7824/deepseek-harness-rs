//! Reuse of verified evidence through bounded model context and current
//! tool preflight. Learning never grants permission or replaces a live guard.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use cordis::{Context, EventOptions, Listener, NextFn, arc, downcast_arc};
use dsh_system_prompt::{AssembleContext, PromptContext, PromptText, SharedAssembly, SystemPrompt};
use dsh_tools::{PreToolDecision, ToolExecution, ToolRuntime};
use serde_json::{Value, json};

use crate::learning::{LearningEntry, LearningStore, rule, workspace_key};

pub const CONTEXT_NAME: &str = "memory:verified-experience";
pub const CONTEXT_BUDGET: usize = 1800;
const MAX_ITEMS: usize = 6;
const MAX_BUDGET: usize = 4000;

#[derive(Clone, Debug, Default)]
pub struct ReuseContext {
    pub workspace: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub tool_names: Vec<String>,
}

struct InjectedContext {
    provider: Option<String>,
    model: Option<String>,
    ids: HashSet<String>,
}

fn verified_suggestion(entry: &LearningEntry) -> Option<&str> {
    if entry.verification.as_deref() == Some("user-confirmed") {
        return (!entry.suggestion.trim().is_empty() && entry.suggestion.chars().count() <= 1000)
            .then_some(entry.suggestion.as_str());
    }
    let (id, _, suggestion, _) = rule(&entry.code, &entry.source);
    (entry.verification.as_deref() == Some("recovered")
        && id == entry.rule_id
        && !suggestion.is_empty())
    .then_some(suggestion)
}

/// The same selection used by prompt assembly. Preview is read-only and is
/// explicitly about the next request, never proof a prior request used it.
pub fn preview(store: &LearningStore, context: &ReuseContext, budget: usize) -> Value {
    let budget = budget.min(MAX_BUDGET);
    let key = workspace_key(&context.workspace);
    let enabled = store.enabled();
    let candidates = if context.workspace.trim().is_empty() || !enabled {
        Vec::new()
    } else {
        store.verified(
            &key,
            None,
            context.provider.as_deref(),
            context.model.as_deref(),
            20,
        )
    };
    let heading = "已验证操作经验（来自本工作目录的执行记录）：以下为有来源的纠正建议，当前工具参数、文件观察状态、权限和运行环境仍须重新校验；经验不授予权限。";
    let mut text = String::new();
    let mut items = Vec::new();
    let mut excluded = Vec::new();
    let mut used = heading.chars().count();
    for entry in candidates {
        let Some(suggestion) = verified_suggestion(&entry) else {
            excluded.push(json!({"id":entry.id,"reason":"unrecognized-rule"}));
            continue;
        };
        let visible = context.tool_names.iter().any(|name| name == &entry.tool);
        if entry.source == "tool" && !visible && entry.rule_id != "tool-availability" {
            excluded.push(json!({"id":entry.id,"reason":"tool-not-visible"}));
            continue;
        }
        let match_reason = if entry.source != "tool" {
            "same-workspace-provider-model"
        } else if visible {
            "same-workspace-tool"
        } else {
            "same-workspace-unavailable-tool"
        };
        let evidence = json!({
            "id":entry.id,"tool":entry.tool,"ruleId":entry.rule_id,"category":entry.category,
            "source":entry.source,"verification":entry.verification,
            "observedProvider":entry.provider,"observedModel":entry.model,
            "suggestion":suggestion,"matchReason":match_reason
        });
        let line = format!(
            "\n{}",
            serde_json::to_string(&evidence).expect("fixed experience evidence JSON")
        );
        let count = line.chars().count();
        if items.len() >= MAX_ITEMS || used.saturating_add(count) > budget {
            excluded.push(json!({"id":entry.id,"reason":"context-budget"}));
            continue;
        }
        if text.is_empty() {
            text.push_str(heading);
        }
        text.push_str(&line);
        used += count;
        items.push(evidence);
    }
    json!({
        "mode":"next-request-preview","contextName":CONTEXT_NAME,"enabled":enabled,
        "workspaceKey":key,"provider":context.provider,"model":context.model,"sessionId":context.session_id,
        "budget":budget,"usedCharacters":text.chars().count(),"maxItems":MAX_ITEMS,
        "items":items,"excluded":excluded,"text":text,
        "selectionRules":["verified-and-enabled","same-workspace","current-tool-or-provider-model","fixed-template-or-user-confirmation","bounded-context"]
    })
}

pub fn install(ctx: &Context, store: Arc<LearningStore>) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|value| value.as_ref().clone())
        .ok_or("experience reuse requires tools")?;
    let prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .map(|value| value.as_ref().clone())
        .ok_or("experience reuse requires systemPrompt")?;
    prompt.context(
        ctx,
        PromptContext {
            name: CONTEXT_NAME.into(),
            order: 106.0,
            // Filled after scoped model selection and queued evidence settle.
            text: PromptText::Static(String::new()),
        },
    );
    let injected = Arc::new(Mutex::new(HashMap::<String, InjectedContext>::new()));
    let assembly_store = store.clone();
    let assembly_injected = injected.clone();
    let assembly_listener: Arc<Listener> = Arc::new(move |_, arguments| {
        let metadata = arguments
            .get(1)
            .and_then(|value| downcast_arc::<AssembleContext>(value));
        let next = arguments
            .last()
            .and_then(|value| downcast_arc::<NextFn>(value));
        let store = assembly_store.clone();
        let injected = assembly_injected.clone();
        Box::pin(async move {
            let Some(next) = next else {
                return None;
            };
            let value = next.call().await;
            let Some(assembly) = downcast_arc::<SharedAssembly>(&value) else {
                return Some(value);
            };
            let Some(metadata) = metadata else {
                return Some(value);
            };
            let snapshot = assembly.snapshot();
            let context = ReuseContext {
                workspace: metadata.field_str("cwd").unwrap_or_default().into(),
                provider: snapshot
                    .variables
                    .get("provider")
                    .and_then(Clone::clone)
                    .or_else(|| metadata.field_str("provider").map(str::to_string)),
                model: snapshot
                    .variables
                    .get("model")
                    .and_then(Clone::clone)
                    .or_else(|| metadata.field_str("model").map(str::to_string)),
                session_id: metadata.field_str("sessionId").map(str::to_string),
                tool_names: snapshot
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect(),
            };
            let settled =
                tokio::time::timeout(std::time::Duration::from_secs(2), store.flush_pending())
                    .await
                    .is_ok_and(|result| result.is_ok());
            let selection = if settled {
                preview(&store, &context, CONTEXT_BUDGET)
            } else {
                json!({"text":"","items":[]})
            };
            let text = selection["text"].as_str().unwrap_or_default().to_string();
            let mut ids = HashSet::new();
            if let Some(target) = assembly
                .0
                .lock()
                .contexts
                .iter_mut()
                .find(|item| item.name == CONTEXT_NAME)
            {
                target.text = text;
                ids.extend(
                    selection["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|entry| entry["id"].as_str())
                        .map(str::to_string),
                );
            }
            if let Some(session) = context.session_id {
                let mut cache = injected.lock().unwrap();
                if cache.len() >= 256
                    && !cache.contains_key(&session)
                    && let Some(oldest) = cache.keys().next().cloned()
                {
                    cache.remove(&oldest);
                }
                cache.insert(
                    session,
                    InjectedContext {
                        provider: context.provider,
                        model: context.model,
                        ids,
                    },
                );
            }
            Some(value)
        })
    });
    futures::executor::block_on(ctx.on(
        "system-prompt/assemble",
        assembly_listener,
        EventOptions::default().prepend(true).global(true),
    ));

    let request_store = store.clone();
    let request_injected = injected.clone();
    let listener: Arc<Listener> = Arc::new(move |_, arguments| {
        let execution = arguments
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|value| value.as_ref().clone());
        let next = arguments
            .last()
            .and_then(|value| downcast_arc::<NextFn>(value));
        let store = store.clone();
        let tools = tools.clone();
        let injected = injected.clone();
        Box::pin(async move {
            if let Some(execution) = execution
                && let Some(agent) = execution.agent.as_ref()
                && let Some(cwd) = agent.session().header().cwd.as_deref()
            {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), store.flush_pending())
                        .await;
                let actual = agent.session().request_context();
                let provider = actual
                    .as_ref()
                    .map(|route| route.provider.as_str())
                    .or(agent.options().provider.as_deref());
                let model = actual
                    .as_ref()
                    .map(|route| route.model.as_str())
                    .or(agent.options().model.as_deref());
                let ids = injected
                    .lock()
                    .unwrap()
                    .get(agent.id().as_str())
                    .filter(|context| {
                        context.provider.as_deref() == provider && context.model.as_deref() == model
                    })
                    .map(|context| context.ids.clone())
                    .unwrap_or_default();
                let entries = store.verified(
                    &workspace_key(cwd),
                    Some(&execution.name),
                    provider,
                    model,
                    MAX_ITEMS,
                );
                let invalid_input = !tools.input_violations(&execution).is_empty();
                for entry in entries {
                    if !ids.contains(&entry.id) || verified_suggestion(&entry).is_none() {
                        continue;
                    }
                    let outcome = if entry.rule_id == "tool-input-schema" && invalid_input {
                        "preflight_blocked"
                    } else {
                        "advisory"
                    };
                    // A delayed optional ledger write cannot indefinitely stall
                    // the authoritative tool safety pipeline.
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(750),
                        store.mark_application(
                            &entry.id,
                            agent.id().as_str(),
                            execution.call_id.as_str(),
                            outcome,
                        ),
                    )
                    .await;
                }
            }
            match next {
                Some(next) => Some(next.call().await),
                None => Some(arc(PreToolDecision::Allow)),
            }
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    ));
    let request_listener: Arc<Listener> = Arc::new(move |_, arguments| {
        let payload = arguments
            .first()
            .and_then(|value| downcast_arc::<dsh_agent::AgentRequestPayload>(value));
        let next = arguments
            .last()
            .and_then(|value| downcast_arc::<NextFn>(value));
        let store = request_store.clone();
        let injected = request_injected.clone();
        Box::pin(async move {
            let Some(next) = next else {
                return None;
            };
            let value = next.call().await;
            if let Some(payload) = payload
                && let Some(route) = downcast_arc::<dsh_llm::LlmCallConfig>(&value)
                && let Some(cwd) = payload.agent.session().header().cwd.as_deref()
            {
                let ids = injected
                    .lock()
                    .unwrap()
                    .get(payload.agent.id().as_str())
                    .filter(|context| {
                        context.provider.as_deref() == Some(route.provider.as_str())
                            && context.model.as_deref() == Some(route.model.as_str())
                    })
                    .map(|context| context.ids.clone())
                    .unwrap_or_default();
                let entries = store.verified(
                    &workspace_key(cwd),
                    None,
                    Some(&route.provider),
                    Some(&route.model),
                    20,
                );
                let call = format!("request:{}:{}", payload.turn, payload.step);
                for entry in entries
                    .into_iter()
                    .filter(|entry| entry.source != "tool" && ids.contains(&entry.id))
                {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(750),
                        store.mark_application(
                            &entry.id,
                            payload.agent.id().as_str(),
                            &call,
                            "advisory",
                        ),
                    )
                    .await;
                }
            }
            Some(value)
        })
    });
    futures::executor::block_on(ctx.on(
        "agent/request",
        request_listener,
        EventOptions::default().prepend(true).global(true),
    ));
    Ok(())
}

#[cfg(test)]
#[path = "experience_reuse_tests.rs"]
mod tests;
