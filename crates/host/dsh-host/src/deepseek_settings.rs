//! Native DeepSeek settings share one validated catalog with the runtime.
use dsh_llm_deepseek::{DeepSeekCatalogModel, DeepSeekConfig, resolve_adapter_options};
use dsh_schemastery::{Data, Schema};
use serde_json::Value;

pub(crate) fn schema() -> Schema {
    let defaults = resolve_adapter_options(&DeepSeekConfig::default()).expect("DeepSeek defaults");
    let model = Schema::object(indexmap::IndexMap::from([
        ("id".into(), Schema::string().required(true)),
        ("enabled".into(), Schema::boolean()),
        ("name".into(), Schema::string()),
        ("description".into(), Schema::string()),
        ("contextWindow".into(), Schema::number().min(1.0).step(1.0)),
        ("maxTokens".into(), Schema::number().min(1.0).step(1.0)),
        ("imageInput".into(), Schema::boolean()),
        ("source".into(), Schema::string()),
        ("accountScope".into(), Schema::string()),
    ]));
    let models = defaults
        .models
        .iter()
        .map(|model| {
            let mut value = serde_json::to_value(model).expect("catalog JSON");
            value
                .as_object_mut()
                .unwrap()
                .retain(|_, value| !value.is_null());
            value
        })
        .collect::<Vec<_>>();
    Schema::object(indexmap::IndexMap::from([
        (
            "modelPreferences".into(),
            super::provider_auth_catalog::preferences_schema(),
        ),
        ("legacyModelScope".into(), Schema::string()),
        ("modelCatalogScope".into(), Schema::string()),
        ("catalogRevision".into(), Schema::string()),
        (
            "apiKeyEnv".into(),
            Schema::string()
                .role("credential-ref", None)
                .default(Data::String(defaults.api_key_env)),
        ),
        (
            "baseURL".into(),
            Schema::string().default(Data::String(defaults.base_url)),
        ),
        (
            "maxTokens".into(),
            Schema::number()
                .min(1.0)
                .step(1.0)
                .default(Data::Number(defaults.max_tokens as f64)),
        ),
        (
            "defaultContextWindow".into(),
            Schema::number()
                .min(1.0)
                .step(1.0)
                .default(Data::Number(defaults.default_context_window as f64)),
        ),
        (
            "models".into(),
            Schema::array(model).default(super::json_to_settings_data(&serde_json::json!(models))),
        ),
    ]))
}

pub(crate) fn config(value: &Value) -> Result<DeepSeekConfig, String> {
    let mut value = value.clone();
    super::model_capabilities::normalize_capacity_numbers(&mut value);
    if let Some(reference) = value.get("apiKeyEnv").and_then(Value::as_str) {
        validate_api_key_reference(reference)?;
    }
    let models: Option<Vec<DeepSeekCatalogModel>> = value
        .get("models")
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|e| format!("DeepSeek model catalog: {e}"))
        })
        .transpose()?;
    if let Some(models) = &models {
        let mut seen = std::collections::HashSet::new();
        for model in models {
            if model.id.trim().is_empty()
                || model.id.trim() != model.id
                || !seen.insert(model.id.clone())
            {
                return Err("DeepSeek model IDs must be non-empty, trimmed and unique".into());
            }
            if model.context_window == Some(0) || model.max_tokens == Some(0) {
                return Err("DeepSeek model capacities must be positive".into());
            }
            if let (Some(output), Some(context)) = (model.max_tokens, model.context_window) {
                if output > context {
                    return Err("DeepSeek maximum output cannot exceed its context window".into());
                }
            }
        }
    }
    Ok(DeepSeekConfig {
        api_key_env: value
            .get("apiKeyEnv")
            .and_then(Value::as_str)
            .map(str::to_string),
        base_url: value
            .get("baseURL")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| std::env::var("DSH_DEEPSEEK_BASE_URL").ok()),
        max_tokens: value.get("maxTokens").and_then(Value::as_u64),
        default_context_window: value.get("defaultContextWindow").and_then(Value::as_u64),
        models,
        ..Default::default()
    })
}

/// OAuth records contain refresh tokens and account metadata, not API keys.
/// Keep them inaccessible to native key/discovery resolvers, including on
/// case-insensitive environment-variable platforms.
pub(crate) fn validate_api_key_reference(reference: &str) -> Result<(), String> {
    if reference
        .trim()
        .to_ascii_uppercase()
        .starts_with("DSH_OAUTH_")
    {
        return Err("OAuth session credentials require their account provider and cannot be used as a DeepSeek API key".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_catalog_settings_affect_visibility_capacity_and_names() {
        use dsh_llm::LlmAdapter;
        let value = serde_json::json!({"models":[{"id":"deepseek-v4-flash","enabled":false},{"id":"custom","name":"Custom model","contextWindow":32000,"maxTokens":4000}]});
        let options = resolve_adapter_options(&config(&value).unwrap()).unwrap();
        let adapter =
            dsh_llm_deepseek::DeepSeekAdapter::new(dsh_llm_deepseek::DeepSeekAdapterOptions {
                options: std::sync::Arc::new(move || Ok(options.clone())),
                resolve_api_key: std::sync::Arc::new(|_| Box::pin(async { Ok(None) })),
                resolve_attachments: None,
                provider_name: None,
                reasoning_wire_format: dsh_llm_deepseek::ReasoningWireFormat::DeepSeek,
            });
        let catalog = adapter.list_models("deepseek").await;
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "Custom model");
        let model = adapter.resolve_model("deepseek", "custom", None).await;
        assert_eq!(model.context.unwrap().context_window, 32000);
        assert_eq!(model.default_max_tokens, Some(4000));
    }
    #[test]
    fn duplicate_ids_and_inverted_capacity_are_rejected() {
        assert!(config(&serde_json::json!({"models":[{"id":"same"},{"id":"same"}]})).is_err());
        assert!(
            config(
                &serde_json::json!({"models":[{"id":"large","contextWindow":100,"maxTokens":200}]})
            )
            .is_err()
        );
    }

    #[test]
    fn oauth_session_references_never_reach_native_key_or_discovery_resolvers() {
        for reference in [
            "DSH_OAUTH_ANTHROPIC_SESSION",
            "dsh_oauth_openai_codex_session",
            " DSH_OAUTH_NOUS_SESSION ",
        ] {
            assert!(validate_api_key_reference(reference).is_err());
            assert!(
                config(
                    &serde_json::json!({"apiKeyEnv": reference,"baseURL":"https://example.com/v1"})
                )
                .is_err()
            );
        }
        assert!(config(&serde_json::json!({"apiKeyEnv":"DEEPSEEK_API_KEY"})).is_ok());
    }
}
