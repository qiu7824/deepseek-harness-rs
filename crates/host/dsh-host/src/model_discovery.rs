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
        .filter_map(Value::as_u64)
        .find(|value| *value > 0)
}

/// Parse model metadata from supported OpenAI-compatible discovery endpoints.
/// Unknown fields are ignored; discovery does not imply a new inference protocol.
pub(crate) fn parse_model_listing(value: &Value) -> Result<Vec<LlmDiscoveredModel>, String> {
    let entries: Vec<(Option<&str>, &Value)> =
        if let Some(data) = value.get("data").and_then(Value::as_array) {
            data.iter().map(|entry| (None, entry)).collect()
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
        let Some(id) = label([entry.get("id")])
            .or_else(|| key.filter(|id| !id.trim().is_empty()).map(str::to_string))
        else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let limit = entry.get("limit");
        models.push(LlmDiscoveredModel {
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
        assert!(parse_model_listing(&json!({"models":[]})).is_err());
    }
}
