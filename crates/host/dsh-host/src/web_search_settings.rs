//! Independent settings for the server-side search provider.
use dsh_schemastery::{Data, Schema};
use serde_json::Value;

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic/v1";
pub(crate) const NAMESPACE: &str = "web-search-deepseek";

pub(crate) struct SearchConfig {
    pub api_key: Option<String>,
    pub api_key_env: String,
    pub base_url: String,
    pub model: String,
    pub api_version: String,
    pub max_tokens: u64,
    pub max_uses: u64,
}

fn schema() -> Schema {
    Schema::object(indexmap::IndexMap::from([
        (
            "mode".into(),
            Schema::union(
                ["deepseek", "hosted"]
                    .into_iter()
                    .map(|s| Schema::constant(Data::String(s.into())))
                    .collect(),
            )
            .default(Data::String("deepseek".into())),
        ),
        (
            "access".into(),
            Schema::union(
                ["live", "cached"]
                    .into_iter()
                    .map(|s| Schema::constant(Data::String(s.into())))
                    .collect(),
            )
            .default(Data::String("live".into())),
        ),
        ("apiKey".into(), Schema::string().role("secret", None)),
        (
            "apiKeyEnv".into(),
            Schema::string()
                .role("credential-ref", None)
                .default(Data::String("DEEPSEEK_API_KEY".into())),
        ),
        ("baseURL".into(), Schema::string()),
        (
            "model".into(),
            Schema::string().default(Data::String("deepseek-v4-flash".into())),
        ),
        (
            "apiVersion".into(),
            Schema::string().default(Data::String("2023-06-01".into())),
        ),
        (
            "maxTokens".into(),
            Schema::number()
                .min(1.0)
                .step(1.0)
                .default(Data::Number(4096.0)),
        ),
        (
            "maxUses".into(),
            Schema::number()
                .min(1.0)
                .step(1.0)
                .default(Data::Number(5.0)),
        ),
    ]))
}

pub(crate) fn config(value: &Value) -> Result<SearchConfig, String> {
    let text = |key: &str, fallback: &str| -> Result<String, String> {
        let value = match value.get(key) {
            None => fallback,
            Some(Value::String(value)) => value,
            _ => return Err(format!("Web search {key} must be text")),
        };
        if value.trim().is_empty() {
            return Err(format!("Web search {key} must not be empty"));
        }
        Ok(value.trim().to_string())
    };
    let positive = |key: &str, fallback| -> Result<u64, String> {
        match value.get(key) {
            None => Ok(fallback),
            Some(value) => value
                .as_u64()
                .filter(|n| *n > 0)
                .or_else(|| {
                    value
                        .as_f64()
                        .filter(|n| {
                            n.is_finite() && *n > 0.0 && n.fract() == 0.0 && *n < u64::MAX as f64
                        })
                        .map(|n| n as u64)
                })
                .ok_or_else(|| format!("Web search {key} must be a positive integer")),
        }
    };
    let base_url = text("baseURL", DEFAULT_BASE_URL)?
        .trim_end_matches('/')
        .to_string();
    let url = reqwest::Url::parse(&base_url)
        .map_err(|_| "Web search Endpoint must be an HTTP(S) API base".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Web search Endpoint must be an HTTP(S) base without embedded credentials, query or fragment".into());
    }
    let api_key_env = text("apiKeyEnv", "DEEPSEEK_API_KEY")?;
    super::deepseek_settings::validate_api_key_reference(&api_key_env)?;
    Ok(SearchConfig {
        api_key: value
            .get("apiKey")
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string),
        api_key_env,
        base_url,
        model: text("model", "deepseek-v4-flash")?,
        api_version: text("apiVersion", "2023-06-01")?,
        max_tokens: positive("maxTokens", 4096)?,
        max_uses: positive("maxUses", 5)?,
    })
}

pub(crate) fn register(
    ctx: &cordis::Context,
    settings: &std::sync::Arc<dsh_settings::SettingsProvider>,
) -> Result<dsh_settings::SettingsScope, String> {
    register_with_base(
        ctx,
        settings,
        std::env::var("DEEPSEEK_SEARCH_BASE_URL").ok().as_deref(),
    )
}

fn register_with_base(
    ctx: &cordis::Context,
    settings: &std::sync::Arc<dsh_settings::SettingsProvider>,
    environment_base: Option<&str>,
) -> Result<dsh_settings::SettingsScope, String> {
    settings.register(ctx, dsh_settings::settings_namespace(NAMESPACE)?, schema(), dsh_settings::SettingsRegisterOptions {
        base: Some(super::json_to_settings_data(&serde_json::json!({"baseURL":environment_base.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_BASE_URL)}))),
        validate: Some(std::sync::Arc::new(|value| config(&value.to_json().ok_or("Web search settings must be JSON-compatible")?).map(|_| ()))),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MemorySettings(parking_lot::Mutex<indexmap::IndexMap<String, Data>>);
    #[async_trait::async_trait]
    impl dsh_settings::SettingsStorage for MemorySettings {
        fn writable(&self) -> bool {
            true
        }
        async fn load(&self) -> Result<indexmap::IndexMap<String, Data>, String> {
            Ok(self.0.lock().clone())
        }
        async fn persist(
            &self,
            namespace: &dsh_settings::SettingsNamespace,
            value: Data,
        ) -> Result<(), String> {
            self.0.lock().insert(namespace.as_str().to_string(), value);
            Ok(())
        }
    }

    #[tokio::test]
    async fn search_settings_are_visible_live_and_independent_of_chat() {
        let ctx = cordis::Context::root();
        let chat = serde_json::json!({"baseURL":"https://chat.example.invalid/v1"});
        let storage = Arc::new(MemorySettings(parking_lot::Mutex::new(
            indexmap::IndexMap::from([(
                "llm-deepseek".into(),
                super::super::json_to_settings_data(&chat),
            )]),
        )));
        let settings = dsh_settings::SettingsProvider::install(&ctx, storage.clone());
        settings.ready().await.unwrap();
        let scope = register_with_base(
            &ctx,
            &settings,
            Some("https://search.example.invalid/anthropic/v1"),
        )
        .unwrap();
        let before = config(&(scope.get)().to_json().unwrap()).unwrap();
        assert_eq!(
            before.base_url,
            "https://search.example.invalid/anthropic/v1"
        );
        (scope.update)(
            serde_json::json!({"baseURL":"https://chosen.example.invalid/v1","maxUses":2}),
        )
        .await
        .unwrap();
        let after = config(&(scope.get)().to_json().unwrap()).unwrap();
        assert_eq!(after.base_url, "https://chosen.example.invalid/v1");
        assert_eq!(after.max_uses, 2);
        let descriptions = settings.describe(dsh_settings::SettingsDescribeOptions {
            redact_secrets: true,
        });
        let section = descriptions
            .iter()
            .find(|row| row.ns.as_str() == NAMESPACE)
            .unwrap();
        assert_eq!(section.applies, dsh_settings::SettingsApplies::Live);
        assert_eq!(section.value.to_json().unwrap()["baseURL"], after.base_url);
        assert_eq!(storage.0.lock()["llm-deepseek"].to_json().unwrap(), chat);
    }
    #[test]
    fn defaults_remain_separate_from_chat_and_reject_oauth_keys() {
        assert_eq!(
            config(&serde_json::json!({})).unwrap().base_url,
            DEFAULT_BASE_URL
        );
        assert!(config(&serde_json::json!({"apiKeyEnv":"DSH_OAUTH_CODEX"})).is_err());
        assert!(
            config(&serde_json::json!({"baseURL":"https://user:password@example.invalid/v1"}))
                .is_err()
        );
        assert!(config(&serde_json::json!({"maxUses":0})).is_err());
    }
}
