use dsh_llm::LlmDiscoveredModel;
use serde_json::Value;
use std::collections::HashSet;

fn label<'a>(values: impl IntoIterator<Item = Option<&'a Value>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn capacity<'a>(values: impl IntoIterator<Item = Option<&'a Value>>) -> Option<u64> {
    values
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|n| {
                        n.is_finite()
                            && *n > 0.0
                            && n.fract() == 0.0
                            && *n <= 9_007_199_254_740_991.0
                    })
                    .map(|n| n as u64)
            })
        })
        .find(|value| *value > 0)
}

/// Parse model metadata from supported OpenAI-compatible discovery endpoints.
/// Unknown fields are ignored; discovery does not imply a new inference protocol.
pub(crate) fn parse_model_listing(value: &Value) -> Result<Vec<LlmDiscoveredModel>, String> {
    let entries: Vec<(Option<&str>, &Value)> =
        if let Some(data) = value.get("data").and_then(Value::as_array) {
            data.iter().map(|entry| (None, entry)).collect()
        } else if let Some(models) = value.get("models").and_then(Value::as_array) {
            models.iter().map(|entry| (None, entry)).collect()
        } else if let Some(models) = value.get("models").and_then(Value::as_object) {
            models
                .iter()
                .map(|(key, entry)| (Some(key.as_str()), entry))
                .collect()
        } else {
            return Err("model discovery response needs a data array or models object".to_string());
        };
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for (key, entry) in entries {
        if !entry.is_object() {
            continue;
        }
        let Some(id) = label([entry.get("id"), entry.get("slug")])
            .or_else(|| key.filter(|id| !id.trim().is_empty()).map(str::to_string))
        else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let limit = entry.get("limit");
        let reasoning_efforts = super::model_capabilities::discovered_efforts(entry);
        let endpoints = entry.get("supported_endpoints").and_then(Value::as_array);
        let api = label([entry.get("api")])
            .filter(|api| {
                [
                    "openai-completions",
                    "openai-responses",
                    "anthropic-messages",
                ]
                .contains(&api.as_str())
            })
            .or_else(|| {
                endpoints.and_then(|values| {
                    if values
                        .iter()
                        .any(|v| v.as_str().is_some_and(|s| s.ends_with("/responses")))
                    {
                        Some("openai-responses".to_string())
                    } else if values
                        .iter()
                        .any(|v| v.as_str().is_some_and(|s| s.ends_with("/chat/completions")))
                    {
                        Some("openai-completions".to_string())
                    } else if values
                        .iter()
                        .any(|v| v.as_str().is_some_and(|s| s.ends_with("/messages")))
                    {
                        Some("anthropic-messages".to_string())
                    } else {
                        None
                    }
                })
            });
        models.push(LlmDiscoveredModel {
            description: label([entry.get("description")]),
            api,
            reasoning_default: super::model_capabilities::reasoning_default(
                entry,
                reasoning_efforts.as_ref(),
            ),
            effort_descriptions: super::model_capabilities::effort_descriptions(
                entry,
                reasoning_efforts.as_ref(),
            ),
            supports_reasoning_summaries: entry
                .get("supports_reasoning_summaries")
                .or_else(|| entry.get("supportsReasoningSummaries"))
                .and_then(Value::as_bool),
            supported_parameters: entry
                .get("supported_parameters")
                .or_else(|| entry.get("supportedParameters"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                }),
            available: entry.get("available").and_then(Value::as_bool).or_else(|| {
                entry
                    .get("visibility")
                    .and_then(Value::as_str)
                    .map(|visibility| !matches!(visibility, "hide" | "hidden" | "none"))
            }),
            reasoning_efforts,
            input: entry
                .get("input_modalities")
                .or_else(|| entry.pointer("/architecture/input_modalities"))
                .and_then(|input| serde_json::from_value(input.clone()).ok()),
            name: label([
                entry.get("name"),
                entry.get("display_name"),
                entry.get("displayName"),
            ]),
            context_window: capacity([
                entry.get("context_window"),
                entry.get("contextWindow"),
                entry.get("context_length"),
                entry.get("max_input_tokens"),
                limit.and_then(|limit| limit.get("context")),
            ]),
            max_tokens: capacity([
                entry.get("max_output_tokens"),
                entry.get("maxOutputTokens"),
                entry.get("max_tokens"),
                entry.get("maxTokens"),
                limit.and_then(|limit| limit.get("output")),
                entry
                    .get("top_provider")
                    .and_then(|provider| provider.get("max_completion_tokens")),
            ])
            .or_else(|| dsh_llm_deepseek::inferred_model_max_tokens(&id)),
            id,
        });
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enriched_map_preserves_endpoint_metadata_and_skips_invalid_values() {
        let models = parse_model_listing(&json!({"models": {
            "custom": {"name":"", "displayName":"Custom", "context_window":0, "limit":{"context":131072,"output":16384}},
            "bad": null
        }})).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "custom");
        assert_eq!(models[0].name.as_deref(), Some("Custom"));
        assert_eq!(models[0].context_window, Some(131072));
        assert_eq!(models[0].max_tokens, Some(16384));
    }

    #[test]
    fn array_deduplicates_and_uses_positive_capacity_fallbacks() {
        let models = parse_model_listing(&json!({"data": [
            {"id":"model", "context_window":-1, "contextWindow":32768, "max_tokens":"invalid", "top_provider":{"max_completion_tokens":4096}},
            {"id":"model"}, {"id":""}, {"name":"missing id"}
        ]})).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].context_window, Some(32768));
        assert_eq!(models[0].max_tokens, Some(4096));
        assert!(
            parse_model_listing(&json!({"models":[]}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn codex_catalog_preserves_exact_reasoning_and_context_metadata() {
        let models = parse_model_listing(&json!({"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5","context_window":272000,
            "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"xhigh"}],"input_modalities":["text","image"]}]})).unwrap();
        assert_eq!(models[0].id, "gpt-5.5");
        assert_eq!(models[0].context_window, Some(272000));
        assert_eq!(
            models[0].reasoning_efforts.as_ref().unwrap()["max"],
            "xhigh"
        );
        assert_eq!(models[0].input.as_ref().unwrap().len(), 2);
    }
}
