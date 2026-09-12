//! Explicit protocol compatibility; configured fields either reach the wire
//! or fail validation before any provider request is sent.
use crate::ResolvedDeepSeekOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingTokenBudgetField {
    #[serde(rename = "thinking_token_budget")]
    ThinkingTokenBudget,
    #[serde(rename = "thinking_budget")]
    ThinkingBudget,
    #[serde(rename = "thinking_budget_tokens")]
    ThinkingBudgetTokens,
}
impl ThinkingTokenBudgetField {
    fn wire(self) -> &'static str {
        match self {
            Self::ThinkingTokenBudget => "thinking_token_budget",
            Self::ThinkingBudget => "thinking_budget",
            Self::ThinkingBudgetTokens => "thinking_budget_tokens",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCompatibility {
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_thinking_token_budget: Option<bool>,
    /// Rust budgets use stable effort ids, before provider wire-name mapping.
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub thinking_budgets: Option<BTreeMap<String, u64>>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub vllm_priority: Option<i32>,
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_max_output_tokens: Option<bool>,
    /// Enable only for an endpoint/model that advertises Responses Lite.
    #[serde(
        default,
        deserialize_with = "non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_responses_lite: Option<bool>,
}
fn non_null<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
impl ProviderCompatibility {
    pub fn merged(&self, model: Option<&Self>) -> Self {
        let Some(model) = model else {
            return self.clone();
        };
        Self {
            thinking_token_budget_field: model
                .thinking_token_budget_field
                .or(self.thinking_token_budget_field),
            supports_thinking_token_budget: model
                .supports_thinking_token_budget
                .or(self.supports_thinking_token_budget),
            thinking_budgets: model
                .thinking_budgets
                .clone()
                .or_else(|| self.thinking_budgets.clone()),
            supports_reasoning_effort: model
                .supports_reasoning_effort
                .or(self.supports_reasoning_effort),
            vllm_priority: model.vllm_priority.or(self.vllm_priority),
            use_responses_lite: model.use_responses_lite.or(self.use_responses_lite),
            supports_max_output_tokens: model
                .supports_max_output_tokens
                .or(self.supports_max_output_tokens),
        }
    }
    fn budget_field(&self) -> Option<ThinkingTokenBudgetField> {
        self.thinking_token_budget_field.or_else(|| {
            (self.supports_thinking_token_budget == Some(true))
                .then_some(ThinkingTokenBudgetField::ThinkingTokenBudget)
        })
    }
    pub fn validate(&self, api: &str) -> Result<(), String> {
        if api != "openai-completions"
            && (self.thinking_token_budget_field.is_some()
                || self.supports_thinking_token_budget.is_some()
                || self.thinking_budgets.is_some()
                || self.vllm_priority.is_some()
                || self.supports_reasoning_effort.is_some())
        {
            return Err("thinkingTokenBudgetField, thinkingBudgets, supportsThinkingTokenBudget, supportsReasoningEffort and vllmPriority require openai-completions".into());
        }
        if api != "openai-responses" && self.use_responses_lite.is_some() {
            return Err("useResponsesLite requires openai-responses".into());
        }
        if api != "openai-responses" && self.supports_max_output_tokens.is_some() {
            return Err("supportsMaxOutputTokens requires openai-responses".into());
        }
        if let Some(budgets) = &self.thinking_budgets {
            if self.budget_field().is_none() {
                return Err("thinkingBudgets requires thinkingTokenBudgetField or supportsThinkingTokenBudget=true".into());
            }
            if budgets.is_empty()
                || budgets.iter().any(|(effort, tokens)| {
                    effort.trim().is_empty()
                        || effort.trim() != effort
                        || effort == "off"
                        || *tokens == 0
                        || *tokens > 1_000_000
                })
            {
                return Err("thinkingBudgets must map non-off effort ids to positive token counts no greater than 1000000".into());
            }
        }
        Ok(())
    }
    pub fn validate_endpoint(&self, api: &str, base_url: &str) -> Result<(), String> {
        self.validate(api)?;
        let codex = reqwest::Url::parse(base_url).ok().is_some_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("chatgpt.com")
                && url.path().trim_end_matches('/') == "/backend-api/codex"
        });
        if codex && self.supports_max_output_tokens == Some(true) {
            return Err("The Codex subscription endpoint does not accept max_output_tokens; remove supportsMaxOutputTokens or set it to false".into());
        }
        Ok(())
    }
}

fn selected(connection: &ResolvedDeepSeekOptions, model: Option<&str>) -> ProviderCompatibility {
    connection.compat.merged(
        connection
            .models
            .iter()
            .find(|entry| Some(entry.id.as_str()) == model)
            .and_then(|entry| entry.compat.as_ref()),
    )
}

pub(crate) fn apply_chat(
    body: &mut Value,
    connection: &ResolvedDeepSeekOptions,
    effort: Option<&str>,
) -> Result<(), dsh_llm::LlmFailure> {
    let compat = selected(connection, body["model"].as_str());
    let effort = effort
        .or_else(|| body["reasoning_effort"].as_str())
        .map(str::to_owned);
    compat
        .validate("openai-completions")
        .map_err(|error| crate::failure(error, "INVALID_CONFIG"))?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| crate::failure("Invalid chat request body", "INVALID_REQUEST"))?;
    if let Some(priority) = compat.vllm_priority {
        object.insert("priority".into(), json!(priority));
    }
    if compat.supports_reasoning_effort == Some(false) {
        object.remove("reasoning_effort");
    }
    if let Some(field) = compat.budget_field() {
        let effort = effort.as_deref().unwrap_or("off");
        if effort != "off" {
            let configured = compat
                .thinking_budgets
                .as_ref()
                .and_then(|budgets| budgets.get(effort))
                .copied();
            // Conservative Rust defaults; an explicit budget map supersedes
            // them and must describe every selectable non-off effort.
            let default = match effort {
                "minimal" => Some(1024),
                "low" => Some(2048),
                "medium" => Some(8192),
                "high" | "xhigh" | "max" => Some(16384),
                _ => None,
            };
            let budget = if compat.thinking_budgets.is_some() {
                configured
            } else {
                default
            }
            .ok_or_else(|| {
                crate::failure(
                    format!("No thinkingBudgets entry for effort {effort:?}"),
                    "UNSUPPORTED_REASONING_EFFORT",
                )
            })?;
            let cap = object
                .get("max_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    crate::failure(
                        "A reasoning token budget requires max_tokens",
                        "INVALID_REQUEST",
                    )
                })?;
            let budget = budget.min(cap.saturating_sub(1024));
            if budget == 0 {
                return Err(crate::failure(
                    "max_tokens must leave at least 1024 tokens for the answer in addition to reasoning",
                    "INVALID_REQUEST",
                ));
            }
            object.insert(field.wire().into(), json!(budget));
        }
    }
    Ok(())
}

pub(crate) fn apply_responses(
    body: &mut Value,
    connection: &ResolvedDeepSeekOptions,
    model: Option<&str>,
) -> Result<(), dsh_llm::LlmFailure> {
    let compat = selected(connection, model);
    compat
        .validate_endpoint("openai-responses", &connection.base_url)
        .map_err(|error| crate::failure(error, "INVALID_CONFIG"))?;
    if compat.supports_max_output_tokens == Some(false) {
        if let Some(body) = body.as_object_mut() {
            body.remove("max_output_tokens");
        }
    }
    if compat.use_responses_lite == Some(true) {
        crate::responses::apply_lite(body)?;
    }
    Ok(())
}

pub(crate) fn responses_lite(connection: &ResolvedDeepSeekOptions, model: Option<&str>) -> bool {
    selected(connection, model).use_responses_lite == Some(true)
}
