//! Current official free pricing, anonymous verification, and local discovery state.
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const PRICING: &str = "https://opencode.ai/docs/zen/";
const DIRECTORY: &str = "https://opencode.ai/zen/v1/models";
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn timestamp(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
        })
        .unwrap_or(0)
}
fn fresh(model: &Value) -> bool {
    let age = now() - timestamp(&model["verifiedAt"]);
    (0..86_400_000).contains(&age)
}

fn merge_preferences(base: &mut Value, user: &Value) {
    if let (Some(base), Some(user)) = (base.as_object_mut(), user.as_object()) {
        for (key, value) in user {
            if let Some(existing) = base.get_mut(key) {
                merge_preferences(existing, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = user.clone();
    }
}

#[derive(Default)]
struct CatalogState {
    models: Vec<Value>,
    updated_at: i64,
    last_attempt: i64,
    refreshing: bool,
    testing: Option<String>,
    error: Option<String>,
}
struct Catalog {
    client: reqwest::Client,
    state: Mutex<CatalogState>,
    path: PathBuf,
    settings: Arc<dsh_settings::SettingsProvider>,
    save_lock: tokio::sync::Mutex<()>,
}

fn strip_tags(value: &str) -> String {
    let tags = Regex::new(r"(?s)<[^>]*>").unwrap();
    let text = tags
        .replace_all(value, "")
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Join the official model-ID/endpoint table with its price table by display label.
fn pricing_catalog(html: &str, live: &HashSet<String>) -> Vec<Value> {
    let row = Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr>").unwrap();
    let cell = Regex::new(r"(?is)<t[dh]\b[^>]*>(.*?)</t[dh]>").unwrap();
    let rows = row
        .captures_iter(html)
        .map(|r| {
            cell.captures_iter(&r[1])
                .map(|c| strip_tags(&c[1]))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let free = rows
        .iter()
        .filter(|r| {
            r.len() >= 4
                && r[1..4]
                    .iter()
                    .all(|price| price.eq_ignore_ascii_case("free"))
        })
        .map(|r| (r[0].as_str(), r))
        .collect::<HashMap<_, _>>();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for row in &rows {
        if row.len() < 3
            || !free.contains_key(row[0].as_str())
            || !live.contains(&row[1])
            || seen.contains(&row[1])
        {
            continue;
        }
        let (api, provider) = match row[2].as_str() {
            "https://opencode.ai/zen/v1/chat/completions" => {
                ("openai-completions", "opencode-free")
            }
            "https://opencode.ai/zen/v1/responses" => {
                ("openai-responses", "opencode-free-responses")
            }
            _ => continue,
        };
        seen.insert(row[1].clone());
        let mut model = json!({"model":row[1],"id":row[1],"name":row[0],"api":api,"provider":provider,"baseUrl":"https://opencode.ai/zen/v1","status":"pending","available":false,"freePricingVerified":true,"catalogAvailable":true,"maxTokens":16384,"pricingEvidence":{"modelId":row[1],"label":row[0],"prices":["Free","Free","Free"],"endpoint":row[2]},"pricingSource":PRICING});
        if row[1] == "ling-3.0-flash-fin-free" {
            model["contextWindow"] = json!(262144);
        }
        result.push(model);
    }
    // A suffix discovers a candidate only; it never authorizes an inference request.
    for id in live
        .iter()
        .filter(|id| id.ends_with("-free") && !seen.contains(*id))
    {
        result.push(json!({"model":id,"id":id,"name":id,"api":null,"provider":null,"status":"pending","available":false,"catalogAvailable":true,"freePricingVerified":false,"reason":"等待官方价格与协议确认"}));
    }
    result.sort_by_key(|m| m["model"].as_str().unwrap_or("").to_string());
    result
}

impl Catalog {
    fn new(
        data: &Path,
        evidence: &Path,
        settings: Arc<dsh_settings::SettingsProvider>,
    ) -> Result<Arc<Self>, String> {
        let path = data.join("free-catalog-state.json");
        let read = |path: &Path| {
            path.metadata()
                .ok()
                .filter(|m| m.len() <= 2 * 1024 * 1024)
                .and_then(|_| std::fs::read(path).ok())
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        };
        let report = read(&path).or_else(|| read(evidence));
        let mut models = report
            .as_ref()
            .and_then(|v| v["models"].as_array())
            .cloned()
            .unwrap_or_default();
        for model in &mut models {
            model["id"] = model["model"].clone();
            if model["status"] == "rate-limited" {
                model["status"] = json!("rate_limited");
            }
            if model["status"] == "pending-verification" {
                model["status"] = json!("pending");
            }
            if model["available"] == true
                && (!fresh(model)
                    || ![
                        "inference",
                        "streaming",
                        "toolCall",
                        "toolResult",
                        "anonymous",
                        "freePricingVerified",
                    ]
                    .iter()
                    .all(|key| model[*key] == true))
            {
                model["available"] = json!(false);
                model["status"] = json!("pending");
            }
            if model["status"] == "testing" {
                model["status"] = json!("pending");
                model["available"] = json!(false);
            }
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("deepseek-harness-rs-free-catalog")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            client,
            state: Mutex::new(CatalogState {
                models,
                ..Default::default()
            }),
            path,
            settings,
            save_lock: tokio::sync::Mutex::new(()),
        }))
    }

    fn view(&self) -> Value {
        let state = self.state.lock();
        let mut models = state.models.clone();
        for model in &mut models {
            if state.testing.as_deref() == model["model"].as_str() {
                model["status"] = json!("testing");
                model["available"] = json!(false);
            } else if model["available"] == true && !fresh(model) {
                model["available"] = json!(false);
                model["status"] = json!("pending");
                model["reason"] = json!("上次检测已超过 24 小时，请重新检测");
            }
        }
        let included = models
            .iter()
            .filter(|m| m["available"] == true)
            .map(|m| json!({"provider":m["provider"],"model":m["model"]}))
            .collect::<Vec<_>>();
        let default = included
            .iter()
            .find(|m| m["model"] == "ling-3.0-flash-fin-free")
            .or_else(|| included.first());
        json!({"models":models,"updatedAt":state.updated_at,"refreshing":state.refreshing,"error":state.error,"pricingSource":PRICING,"includedModels":included,"defaultModel":default})
    }

    async fn save(&self) {
        let _write = self.save_lock.lock().await;
        let value = self.view();
        if let Err(error) = dsh_atomic_write::write_file_atomic(
            &self.path,
            value.to_string().as_bytes(),
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        {
            self.state.lock().error = Some(format!("免费模型检测状态未能保存：{error}"));
        }
    }

    async fn fetch_catalog(&self) -> Result<Vec<Value>, String> {
        let fetch = |url| async move {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|_| "无法连接官方模型目录".to_string())?;
            if !response.status().is_success() {
                return Err(format!("官方目录 HTTP {}", response.status().as_u16()));
            }
            if response
                .content_length()
                .is_some_and(|n| n > 5 * 1024 * 1024)
            {
                return Err("官方目录超过读取上限".into());
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|_| "官方目录读取中断".to_string())?;
            if bytes.len() > 5 * 1024 * 1024 {
                return Err("官方目录超过读取上限".into());
            }
            Ok::<_, String>(bytes)
        };
        let (pricing, directory) = tokio::join!(fetch(PRICING), fetch(DIRECTORY));
        let directory: Value =
            serde_json::from_slice(&directory?).map_err(|_| "官方模型目录 JSON 无效")?;
        let live = directory["data"]
            .as_array()
            .ok_or("官方模型目录缺少 data")?
            .iter()
            .filter_map(|m| m["id"].as_str().map(str::to_string))
            .collect::<HashSet<_>>();
        let pricing = pricing?;
        let mut models = pricing_catalog(
            std::str::from_utf8(&pricing).map_err(|_| "官方定价页面编码无效")?,
            &live,
        );
        if !models
            .iter()
            .any(|model| model["freePricingVerified"] == true)
        {
            return Err("未能从官方定价表确认免费模型，已保留上次目录".into());
        }
        for old in &self.state.lock().models {
            if old["model"].as_str().is_some_and(|id| live.contains(id))
                && !models.iter().any(|m| m["model"] == old["model"])
            {
                models.push(json!({"model":old["model"],"id":old["model"],"name":old["name"],"status":"pending","available":false,"catalogAvailable":true,"freePricingVerified":false,"reason":"当前目录仍有此模型，等待免费价格确认"}));
            }
        }
        Ok(models)
    }

    fn merge(&self, mut models: Vec<Value>) {
        let mut state = self.state.lock();
        for model in &mut models {
            if let Some(old) = state.models.iter().find(|old| {
                old["model"] == model["model"]
                    && old["api"] == model["api"]
                    && model["freePricingVerified"] == true
            }) {
                for key in [
                    "verifiedAt",
                    "status",
                    "available",
                    "reason",
                    "inference",
                    "streaming",
                    "toolCall",
                    "toolResult",
                    "anonymous",
                    "harnessVerified",
                    "probeSource",
                ] {
                    if let Some(value) = old.get(key) {
                        model[key] = value.clone();
                    }
                }
            }
        }
        for old in &state.models {
            if !models.iter().any(|m| m["model"] == old["model"]) {
                let mut retired = old.clone();
                retired["status"] = json!("retired");
                retired["available"] = json!(false);
                retired["catalogAvailable"] = json!(false);
                retired["freePricingVerified"] = json!(false);
                retired["reason"] = json!("当前官方目录或免费价格表已移除该模型");
                models.push(retired);
            }
        }
        state.models = models;
        state.updated_at = now();
        state.error = None;
    }

    fn refresh(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.refreshing {
                return;
            }
            state.refreshing = true;
            state.last_attempt = now();
        }
        let service = self.clone();
        tokio::spawn(async move {
            match service.fetch_catalog().await {
                Ok(models) => service.merge(models),
                Err(error) => service.state.lock().error = Some(error),
            }
            service.state.lock().refreshing = false;
            service.save().await;
        });
    }

    fn test(self: &Arc<Self>, model: &str) -> Result<(), String> {
        {
            let mut state = self.state.lock();
            if state.testing.is_some() {
                return Err("已有免费模型正在检测，请等待完成".into());
            }
            let entry = state
                .models
                .iter_mut()
                .find(|m| m["model"] == model)
                .ok_or("模型不在免费目录中")?;
            if entry["freePricingVerified"] != true || entry["catalogAvailable"] == false {
                return Err("此模型没有当前免费价格证明".into());
            }
            entry["status"] = json!("testing");
            entry["available"] = json!(false);
            entry["reason"] = Value::Null;
            state.testing = Some(model.to_string());
        }
        let service = self.clone();
        let model = model.to_string();
        tokio::spawn(async move {
            let result = async {
                // Recheck pricing immediately before every inference; persisted flags are not authority.
                let catalog = service.fetch_catalog().await?;
                let entry = catalog
                    .iter()
                    .find(|m| m["model"] == model)
                    .cloned()
                    .ok_or("官方目录当前不再确认此模型免费")?;
                service.merge(catalog);
                let flags = super::free_probe::verify(
                    &service.client,
                    &model,
                    entry["api"].as_str().ok_or("缺少协议")?,
                )
                .await?;
                Ok::<_, String>((flags, entry))
            }
            .await;
            {
                let mut state = service.state.lock();
                state.testing = None;
                if let Some(entry) = state.models.iter_mut().find(|m| m["model"] == model) {
                    entry["verifiedAt"] = json!(now());
                    if let Some(fields) = entry.as_object_mut() {
                        fields.remove("harnessVerified");
                        fields.remove("binarySha256");
                    }
                    match result {
                        Ok((flags, checked))
                            if entry["freePricingVerified"] == true
                                && entry["catalogAvailable"] == true
                                && entry["api"] == checked["api"]
                                && entry["pricingEvidence"] == checked["pricingEvidence"] =>
                        {
                            for (key, value) in flags.as_object().unwrap() {
                                entry[key] = value.clone();
                            }
                            entry["status"] = json!("available");
                            entry["available"] = json!(true);
                            entry["reason"] = Value::Null;
                        }
                        Ok(_) => {
                            entry["available"] = json!(false);
                            entry["status"] = json!(if entry["catalogAvailable"] == false {
                                "retired"
                            } else {
                                "pending"
                            });
                            entry["reason"] = json!("检测期间目录发生变化，请刷新后重试");
                        }
                        Err(error) => {
                            entry["status"] = json!(if entry["catalogAvailable"] == false {
                                "retired"
                            } else if error.contains("HTTP 429")
                                || error.to_ascii_lowercase().contains("rate_limit")
                            {
                                "rate_limited"
                            } else {
                                "unavailable"
                            });
                            entry["available"] = json!(false);
                            entry["reason"] = json!(error);
                        }
                    }
                }
            }
            service.save().await;
        });
        Ok(())
    }

    async fn enable(&self, id: &str) -> Result<Value, String> {
        let verified = self
            .state
            .lock()
            .models
            .iter()
            .find(|m| m["model"] == id)
            .cloned()
            .ok_or("模型不在免费目录中")?;
        if verified["available"] != true || !fresh(&verified) {
            return Err("请先完成免费模型连通性检测".into());
        }
        let current = self.fetch_catalog().await?;
        let model = current
            .iter()
            .find(|m| m["model"] == id)
            .ok_or("当前官方价格表不再确认此模型免费")?;
        let provider = model["provider"].as_str().ok_or("缺少提供商")?;
        let ns = dsh_settings::settings_namespace("llm-pi-ai").map_err(|e| e.to_string())?;
        let descriptor = self
            .settings
            .describe(dsh_settings::SettingsDescribeOptions {
                redact_secrets: false,
            })
            .into_iter()
            .find(|d| d.ns == ns)
            .ok_or("模型设置不可用")?;
        let value = descriptor.value.to_json().ok_or("模型设置无效")?;
        let mut profile = value
            .get("providers")
            .and_then(|v| v.get(provider))
            .cloned()
            .unwrap_or(json!({}));
        let base = descriptor
            .base
            .as_ref()
            .and_then(|v| v.to_json())
            .and_then(|v| v.get("providers").and_then(|v| v.get(provider)).cloned())
            .unwrap_or(json!({}));
        let user = descriptor
            .user
            .as_ref()
            .and_then(|v| v.to_json())
            .and_then(|v| v.get("providers").and_then(|v| v.get(provider)).cloned())
            .unwrap_or(json!({}));
        // Write sparse user/base declarations, never schema-materialized empty capability maps.
        profile["models"] = user
            .get("models")
            .or_else(|| base.get("models"))
            .cloned()
            .unwrap_or(json!([]));
        let mut preferences = base.get("modelPreferences").cloned().unwrap_or(json!({}));
        if let Some(user) = user.get("modelPreferences") {
            merge_preferences(&mut preferences, user);
        }
        profile["modelPreferences"] = preferences;
        if profile.get("authProvider").is_some()
            || profile
                .get("apiKeyEnv")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
            || profile.get("api").is_some_and(|api| *api != model["api"])
            || profile
                .get("baseURL")
                .and_then(Value::as_str)
                .is_some_and(|v| v != "https://opencode.ai/zen/v1")
            || profile
                .get("headers")
                .and_then(Value::as_object)
                .is_some_and(|headers| {
                    headers.keys().any(|key| {
                        key.eq_ignore_ascii_case("authorization")
                            || key.eq_ignore_ascii_case("x-api-key")
                    })
                })
        {
            return Err("同名提供商已有其他配置，请先在模型管理中检查".into());
        }
        let mut models = profile
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let row =
            json!({"id":id,"name":model["name"],"maxTokens":model["maxTokens"],"enabled":true});
        if let Some(existing) = models.iter_mut().find(|m| m["id"] == id) {
            existing["enabled"] = json!(true);
        } else {
            let mut row = row;
            if let Some(capacity) = model.get("contextWindow") {
                row["contextWindow"] = capacity.clone();
            }
            models.push(row);
        }
        profile["models"] = json!(models);
        profile["keyless"] = json!(true);
        profile["baseURL"] = json!("https://opencode.ai/zen/v1");
        profile["api"] = model["api"].clone();
        if profile.get("displayName").is_none() {
            profile["displayName"] = json!(if provider.ends_with("responses") {
                "OpenCode 免费模型 · Responses"
            } else {
                "OpenCode 免费模型"
            });
        }
        let scope = format!(
            "anonymous-{}",
            super::provider_auth_catalog::key("https://opencode.ai/zen/v1")
        );
        if profile.get("modelPreferences").is_none() {
            profile["modelPreferences"] = json!({});
        }
        if profile["modelPreferences"].get(&scope).is_none() {
            profile["modelPreferences"][&scope] = json!({});
        }
        if profile["modelPreferences"][&scope].get(id).is_none() {
            profile["modelPreferences"][&scope][id] = json!({});
        }
        profile["modelPreferences"][&scope][id]["enabled"] = json!(true);
        // A stale UI/parallel edit causes a revision conflict instead of replacing its model list.
        self.settings
            .update(
                &ns,
                json!({"providers":{provider:profile}}),
                Some(descriptor.revision),
            )
            .await?;
        Ok(json!({"enabled":true,"provider":provider,"model":id}))
    }
}

pub(super) fn register(
    server: &Arc<WebServer>,
    data: &Path,
    evidence: &Path,
    settings: Arc<dsh_settings::SettingsProvider>,
) -> Result<RouteDisposer, String> {
    let service = Catalog::new(data, evidence, settings)?;
    Ok(server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-free".into(),
        handler: Arc::new(move |request| {
            let service = service.clone();
            Box::pin(async move {
                let action = request
                    .uri()
                    .path()
                    .trim_start_matches("/__dsh-free/")
                    .to_string();
                let method = request.method().clone();
                let (status, body) = if !super::trusted_web_request(&request, false) {
                    (403, json!({"error":"forbidden"}))
                } else if method == http::Method::GET && action == "models" {
                    let initial = {
                        let state = service.state.lock();
                        state.updated_at == 0 && now() - state.last_attempt >= 60_000
                    };
                    if initial {
                        service.refresh();
                    }
                    (200, service.view())
                } else if method == http::Method::POST && action == "refresh" {
                    service.refresh();
                    (202, service.view())
                } else if method == http::Method::POST && action == "enable" {
                    let body =
                        axum::body::to_bytes(axum::body::Body::new(request.into_body()), 8192)
                            .await
                            .ok()
                            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                    match body.as_ref().and_then(|v| v["model"].as_str()) {
                        Some(id) => match service.enable(id).await {
                            Ok(value) => (200, value),
                            Err(error) => (400, json!({"error":error,"message":error})),
                        },
                        None => (400, json!({"error":"缺少模型 ID"})),
                    }
                } else if method == http::Method::POST && action == "test" {
                    let body =
                        axum::body::to_bytes(axum::body::Body::new(request.into_body()), 8192)
                            .await
                            .ok()
                            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                    match body
                        .as_ref()
                        .and_then(|v| v["model"].as_str())
                        .ok_or("缺少模型 ID".to_string())
                        .and_then(|id| service.test(id))
                    {
                        Ok(()) => (202, service.view()),
                        Err(error) => (400, json!({"error":error,"message":error})),
                    }
                } else {
                    (405, json!({"error":"method not allowed"}))
                };
                Ok(http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json; charset=utf-8")
                    .header("cache-control", "no-store")
                    .body(axum::body::Body::from(body.to_string()))
                    .expect("free catalog response"))
            })
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_ids_with_all_three_free_prices_are_admitted() {
        let html = r#"<table><tr><th>Name</th><th>ID</th><th>Endpoint</th></tr><tr><td>Model A</td><td>a-free</td><td>https://opencode.ai/zen/v1/responses</td></tr><tr><td>Model B</td><td>b-free</td><td>https://opencode.ai/zen/v1/chat/completions</td></tr></table><table><tr><td>Model A</td><td>Free</td><td>Free</td><td>Free</td></tr><tr><td>Model B</td><td>Free</td><td>$1</td><td>Free</td></tr></table>"#;
        let rows = pricing_catalog(html, &HashSet::from(["a-free".into(), "b-free".into()]));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["api"], "openai-responses");
        assert_eq!(rows[0]["status"], "pending");
        assert!(rows[0].get("contextWindow").is_none());
        assert_eq!(
            rows.iter()
                .filter(|m| m["freePricingVerified"] == true)
                .count(),
            1
        );
        assert_eq!(
            pricing_catalog(html, &HashSet::from(["b-free".into()]))[0]["freePricingVerified"],
            false
        );
    }
}
