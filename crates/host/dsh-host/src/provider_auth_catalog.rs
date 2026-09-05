//! Account-scoped remote catalogs. Provider metadata is cached independently
//! from editable model preferences and manually declared models.
use dsh_llm::LlmDiscoveredModel;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub(crate) fn key(value: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(value.as_bytes()))
}
pub(crate) fn preferences_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::{Data, Schema};
    let fields = indexmap::IndexMap::from([
        ("enabled".into(), Schema::boolean()),
        ("name".into(), Schema::string()),
        ("description".into(), Schema::string()),
        (
            "api".into(),
            Schema::union(
                [
                    "openai-completions",
                    "openai-responses",
                    "anthropic-messages",
                ]
                .into_iter()
                .map(|s| Schema::constant(Data::String(s.into())))
                .collect(),
            ),
        ),
        ("contextWindow".into(), Schema::number().min(1.0).step(1.0)),
        ("maxTokens".into(), Schema::number().min(1.0).step(1.0)),
        ("input".into(), Schema::array(Schema::string())),
        ("imageInput".into(), Schema::boolean()),
        (
            "reasoningEfforts".into(),
            Schema::union(vec![
                Schema::constant(Data::Bool(false)),
                Schema::dict(
                    Schema::union(vec![Schema::string(), Schema::constant(Data::Null)]),
                    None,
                ),
            ]),
        ),
        ("reasoningDefault".into(), Schema::string()),
        (
            "effortDescriptions".into(),
            Schema::dict(Schema::string(), None),
        ),
    ]);
    Schema::dict(Schema::dict(Schema::object(fields), None), None)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Catalog {
    pub provider: String,
    pub account_scope: String,
    pub status: String,
    pub source: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub updated_at: Option<u64>,
    pub error: Option<String>,
    pub models: Vec<LlmDiscoveredModel>,
}
impl Catalog {
    pub fn empty(provider: &str, scope: &str) -> Self {
        Self {
            provider: provider.into(),
            account_scope: scope.into(),
            status: "not-synced".into(),
            source: "remote".into(),
            endpoint: None,
            updated_at: None,
            error: None,
            models: Vec::new(),
        }
    }
    pub fn status_value(&self) -> Value {
        json!({"status":self.status,"source":self.source,"endpoint":self.endpoint,"updatedAt":self.updated_at,"error":self.error,"count":self.models.iter().filter(|m|m.available!=Some(false)).count()})
    }
}
pub(crate) struct CatalogStore {
    root: parking_lot::RwLock<PathBuf>,
    records: parking_lot::Mutex<HashMap<(String, String), Catalog>>,
    bindings: parking_lot::RwLock<HashMap<String, String>>,
    pub locks: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}
impl CatalogStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: parking_lot::RwLock::new(root),
            records: Default::default(),
            bindings: Default::default(),
            locks: Default::default(),
        }
    }
    pub fn set_root(&self, root: PathBuf) {
        *self.root.write() = root;
    }
    pub fn bind(&self, provider: &str, scope: &str) {
        self.bindings.write().insert(provider.into(), scope.into());
    }
    pub fn unbind(&self, provider: &str) {
        self.bindings.write().remove(provider);
    }
    pub fn scope(&self, provider: &str) -> Option<String> {
        self.bindings.read().get(provider).cloned()
    }
    fn path(&self, provider: &str, scope: &str) -> PathBuf {
        self.root
            .read()
            .join(format!("{}.json", key(&format!("{provider}\0{scope}"))))
    }
    pub fn get(&self, provider: &str, scope: &str) -> Catalog {
        let cache_key = (provider.to_string(), scope.to_string());
        if let Some(value) = self.records.lock().get(&cache_key).cloned() {
            return value;
        }
        let path = self.path(provider, scope);
        let loaded = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 8 * 1024 * 1024)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|raw| serde_json::from_slice::<Catalog>(&raw).ok())
            .filter(|c| c.provider == provider && c.account_scope == scope)
            .unwrap_or_else(|| Catalog::empty(provider, scope));
        self.records.lock().insert(cache_key, loaded.clone());
        loaded
    }
    pub fn mark_syncing(&self, provider: &str, scope: &str) {
        let mut value = self.get(provider, scope);
        value.status = "syncing".into();
        value.error = None;
        self.records
            .lock()
            .insert((provider.into(), scope.into()), value);
    }
    pub async fn store(&self, value: Catalog) -> Result<(), String> {
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("模型目录超过缓存容量".into());
        }
        dsh_atomic_write::write_file_atomic(
            &self.path(&value.provider, &value.account_scope),
            &bytes,
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|_| "无法保存模型目录缓存".to_string())?;
        self.records
            .lock()
            .insert((value.provider.clone(), value.account_scope.clone()), value);
        Ok(())
    }
    pub async fn fail(&self, provider: &str, scope: &str, error: &str) {
        let mut value = self.get(provider, scope);
        value.status = "error".into();
        value.error = Some(error.to_string());
        self.records
            .lock()
            .insert((provider.into(), scope.into()), value.clone());
        let _ = self.store(value).await;
    }
}

pub(crate) const OVERRIDE_FIELDS: &[&str] = &[
    "enabled",
    "name",
    "description",
    "api",
    "contextWindow",
    "maxTokens",
    "input",
    "imageInput",
    "reasoningEfforts",
    "reasoningDefault",
    "effortDescriptions",
];

/// All provider types share the same visible model rows and field override
/// semantics. An account's old remote catalog never becomes another account's
/// manually configured list.
pub(crate) fn merge_models(profile: &Value, catalog: &Catalog, native: bool) -> Vec<Value> {
    let mut rows = indexmap::IndexMap::<String, Value>::new();
    for model in &catalog.models {
        let mut row = serde_json::to_value(model).expect("model metadata JSON");
        row["source"] = json!(catalog.source);
        row["availability"] = json!(if model.available == Some(false) {
            "unavailable"
        } else {
            "available"
        });
        row["enabled"] = json!(model.available != Some(false));
        rows.insert(model.id.clone(), row);
    }
    let legacy_scope = profile.get("legacyModelScope").and_then(Value::as_str);
    for model in profile
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = model
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        let manual = model.get("source").and_then(Value::as_str) == Some("manual");
        let model_scope = model
            .get("accountScope")
            .and_then(Value::as_str)
            .or(legacy_scope);
        if model_scope.is_some_and(|scope| scope != catalog.account_scope) {
            continue;
        }
        if let Some(row) = rows.get_mut(id) {
            // Legacy rows predate provenance tracking. Preserve their explicit
            // settings as user overrides until individually reset.
            if manual || catalog.updated_at.is_none() || model_scope.is_none() {
                for field in OVERRIDE_FIELDS {
                    if let Some(value) = model.get(*field).filter(|v| !v.is_null()) {
                        row[*field] = value.clone();
                    }
                }
            }
        } else {
            let mut row = model.clone();
            let available = manual
                || native
                || profile
                    .get("authProvider")
                    .and_then(Value::as_str)
                    .is_none();
            row["source"] = json!(if manual {
                "manual"
            } else if native {
                "builtin"
            } else {
                "legacy"
            });
            row["available"] = json!(available);
            row["availability"] = json!(if available {
                "available"
            } else {
                "unavailable"
            });
            if !available {
                row["enabled"] = json!(false);
            } else if row.get("enabled").is_none_or(Value::is_null) {
                row["enabled"] = json!(true);
            }
            rows.insert(id.into(), row);
        }
    }
    let preferences = profile
        .get("modelPreferences")
        .and_then(|v| v.get(&catalog.account_scope))
        .and_then(Value::as_object);
    for (id, row) in &mut rows {
        if let Some(values) = preferences
            .and_then(|p| p.get(id))
            .and_then(Value::as_object)
        {
            for field in OVERRIDE_FIELDS {
                if let Some(value) = values.get(*field).filter(|v| !v.is_null()) {
                    row[*field] = value.clone();
                }
            }
            row["overriddenFields"] = json!(values.keys().collect::<Vec<_>>());
        } else {
            row["overriddenFields"] = json!([]);
        }
        if row.get("available") == Some(&Value::Bool(false)) {
            row["enabled"] = json!(false);
        }
        if row.get("input").is_none()
            && let Some(image) = row.get("imageInput").and_then(Value::as_bool)
        {
            row["input"] = if image {
                json!(["text", "image"])
            } else {
                json!(["text"])
            };
        }
        enrich_capabilities(row, profile);
    }
    rows.into_values().collect()
}

pub(crate) fn enrich_capabilities(row: &mut Value, profile: &Value) {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut sources = serde_json::Map::new();
    if row.get("reasoningEfforts").is_none_or(Value::is_null)
        && let Some(levels) = crate::model_capabilities::inferred_efforts(&id)
    {
        row["reasoningEfforts"] = json!(levels);
        sources.insert("reasoning".into(), json!("official-reference"));
    }
    if profile.get("api").and_then(Value::as_str) != Some("anthropic-messages") {
        let official_openai = profile
            .get("baseURL")
            .and_then(Value::as_str)
            .and_then(|base| reqwest::Url::parse(base).ok())
            .is_some_and(|url| matches!(url.host_str(), Some("api.openai.com" | "chatgpt.com")));
        if (official_openai || profile.get("api").is_none_or(Value::is_null))
            && row.get("api").is_none_or(Value::is_null)
            && let Some(api) = crate::model_capabilities::documented_api(&id)
        {
            row["api"] = json!(api);
            sources.insert("api".into(), json!("official-reference"));
        }
        let (context, output) = crate::model_capabilities::documented_capacities(&id);
        for (field, value) in [("contextWindow", context), ("maxTokens", output)] {
            if row.get(field).is_none_or(Value::is_null)
                && let Some(value) = value
            {
                row[field] = json!(value);
                sources.insert(field.into(), json!("official-reference"));
            }
        }
        if id == "gpt-6-astra" && row.get("supportedParameters").is_none_or(Value::is_null) {
            row["supportedParameters"] = json!(["max_output_tokens", "reasoning"]);
        }
    }
    if let Some(levels) = row.get("reasoningEfforts").and_then(Value::as_object) {
        let descriptions = row.get("effortDescriptions");
        let choices=levels.iter().map(|(id,wire)|json!({"id":id,"name":match id.as_str(){"off"=>"Off","low"=>"Low","medium"=>"Medium","high"=>"High","xhigh"=>"Extra High","max" if wire.as_str()==Some("xhigh")=>"Extra High","max"=>"Maximum",other=>other},"description":descriptions.and_then(|d|d.get(id))})).collect::<Vec<_>>();
        let default = row
            .get("reasoningDefault")
            .and_then(Value::as_str)
            .filter(|id| levels.contains_key(*id));
        let mut reasoning = json!({"efforts":choices});
        if let Some(default) = default {
            reasoning["defaultEffort"] = json!(default);
        }
        row["reasoning"] = reasoning;
    }
    row["capabilitySources"] = Value::Object(sources);
}

pub(crate) fn migrate_legacy_preferences(profile: &mut Value, scope: &str) {
    if profile
        .get("legacyModelScope")
        .and_then(Value::as_str)
        .is_some()
    {
        return;
    }
    let legacy = profile
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    profile["legacyModelScope"] = json!(scope);
    if profile.get("modelPreferences").is_none() {
        profile["modelPreferences"] = json!({});
    }
    if profile["modelPreferences"].get(scope).is_none() {
        profile["modelPreferences"][scope] = json!({});
    }
    for model in legacy {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let mut values = serde_json::Map::new();
        for field in OVERRIDE_FIELDS {
            if let Some(value) = model.get(*field).filter(|v| !v.is_null()) {
                values.insert((*field).into(), value.clone());
            }
        }
        if !values.is_empty() {
            if profile["modelPreferences"][scope].get(id).is_none() {
                profile["modelPreferences"][scope][id] = json!({});
            }
            for (field, value) in values {
                if profile["modelPreferences"][scope][id].get(&field).is_none() {
                    profile["modelPreferences"][scope][id][&field] = value;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog(scope: &str, models: Value) -> Catalog {
        Catalog {
            provider: "account".into(),
            account_scope: scope.into(),
            status: "synced".into(),
            source: "remote".into(),
            endpoint: None,
            updated_at: Some(1),
            error: None,
            models: serde_json::from_value(models).unwrap(),
        }
    }
    #[test]
    fn remote_updates_preserve_preferences_without_pinning_provider_capabilities() {
        let profile = json!({"models":[],"modelPreferences":{"alice":{"gpt-6":{"enabled":false,"name":"Alias"}}}});
        let a = merge_models(
            &profile,
            &catalog(
                "alice",
                json!([{"id":"gpt-6","contextWindow":1000,"reasoningEfforts":{"high":"high"}}]),
            ),
            false,
        );
        let b = merge_models(
            &profile,
            &catalog(
                "alice",
                json!([{"id":"gpt-6","contextWindow":2000,"reasoningEfforts":{"high":"high","max":"max"}},{"id":"new-model"}]),
            ),
            false,
        );
        assert_eq!(a[0]["name"], "Alias");
        assert_eq!(b[0]["enabled"], false);
        assert_eq!(b[0]["contextWindow"], 2000);
        assert_eq!(b.len(), 2);
        let other = merge_models(&profile, &catalog("bob", json!([{"id":"gpt-6"}])), false);
        assert_eq!(other[0]["enabled"], true);
        assert!(other[0].get("name").is_none());
    }
    #[test]
    fn legacy_and_manual_models_do_not_cross_accounts() {
        let mut profile = json!({"authProvider":"openai-codex","models":[{"id":"gpt-5.4","enabled":false},{"id":"private","source":"manual","accountScope":"alice"}]});
        migrate_legacy_preferences(&mut profile, "alice");
        assert_eq!(
            merge_models(
                &profile,
                &catalog("alice", json!([{"id":"gpt-5.4"},{"id":"gpt-6"}])),
                false
            )
            .len(),
            3
        );
        let other = merge_models(&profile, &catalog("bob", json!([{"id":"gpt-6"}])), false);
        assert_eq!(other.len(), 1);
        assert_eq!(other[0]["id"], "gpt-6");
    }
    #[test]
    fn documented_protocol_fallback_respects_gateway_and_model_overrides() {
        let source = catalog("account", json!([{"id":"gpt-6-astra"}]));
        let official = merge_models(
            &json!({"baseURL":"https://api.openai.com/v1","api":"openai-completions"}),
            &source,
            false,
        );
        assert_eq!(official[0]["api"], "openai-responses");
        assert_eq!(official[0]["reasoningEfforts"]["xhigh"], "xhigh");
        assert_eq!(official[0]["reasoningEfforts"]["max"], "max");
        let gateway = merge_models(
            &json!({"baseURL":"https://gateway.example/v1","api":"openai-completions"}),
            &source,
            false,
        );
        assert!(gateway[0].get("api").is_none());
        let explicit = merge_models(
            &json!({"baseURL":"https://api.openai.com/v1","api":"openai-completions","modelPreferences":{"account":{"gpt-6-astra":{"api":"openai-completions"}}}}),
            &source,
            false,
        );
        assert_eq!(explicit[0]["api"], "openai-completions");
    }
}
