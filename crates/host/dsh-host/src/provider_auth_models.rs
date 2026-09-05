use super::*;
use crate::provider_auth_catalog::{Catalog, merge_models, migrate_legacy_preferences};

pub(crate) struct CatalogRequest {
    pub url: reqwest::Url,
    pub headers: reqwest::header::HeaderMap,
}
pub(crate) type CatalogTransport =
    Arc<dyn Fn(CatalogRequest) -> cordis::BoxFuture<'static, Result<Value, String>> + Send + Sync>;

pub(crate) fn catalog_url(profile: &Value) -> Result<reqwest::Url, String> {
    let base = profile
        .get("baseURL")
        .and_then(Value::as_str)
        .ok_or("供应商没有配置服务地址")?;
    let mut url = reqwest::Url::parse(base).map_err(|_| "供应商地址无效")?;
    let host = url.host_str().ok_or("供应商地址无效")?.to_string();
    if url.scheme() != "https"
        && !(url.scheme() == "http" && matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"))
    {
        return Err("目录发现要求HTTPS，本机测试地址可使用HTTP".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("供应商地址不得包含凭据".into());
    }
    let auth = profile.get("authProvider").and_then(Value::as_str);
    if auth == Some("openai-codex") {
        if !super::valid_profile("openai-codex", base, "openai-responses") {
            return Err("Codex目录仅允许官方账号地址".into());
        }
        url.set_path("/backend-api/codex/models");
        url.set_query(Some("client_version=1.0.0"));
    } else if matches!(host.as_str(), "api.minimax.io" | "api.minimaxi.com") {
        // MiniMax exposes its catalog through the OpenAI model-list endpoint,
        // independently of its Anthropic inference endpoint.
        url.set_path("/v1/models");
        url.set_query(None);
    } else {
        let mut path = url.path().trim_end_matches('/').to_string();
        for suffix in ["/chat/completions", "/responses", "/messages"] {
            if path.ends_with(suffix) {
                path.truncate(path.len() - suffix.len());
                break;
            }
        }
        if profile.get("api").and_then(Value::as_str) == Some("anthropic-messages")
            && !path.ends_with("/v1")
        {
            path.push_str("/v1");
        }
        if !path.ends_with("/models") {
            path.push_str("/models");
        }
        url.set_path(&path);
        url.set_query(None);
    }
    url.set_fragment(None);
    Ok(url)
}

impl AccountAuth {
    pub(crate) fn set_catalog_root(&self, root: std::path::PathBuf) {
        self.catalogs.set_root(root);
    }
    fn profile_address(route: &str) -> (String, Vec<String>) {
        if route == "deepseek-official" {
            ("llm-deepseek".into(), Vec::new())
        } else {
            ("llm-pi-ai".into(), vec!["providers".into(), route.into()])
        }
    }
    fn model_profile(&self, route: &str) -> Result<Value, String> {
        self.profile_snapshot(route, false)
            .map(|(profile, _)| profile)
    }
    pub(super) fn profile_snapshot(
        &self,
        route: &str,
        create: bool,
    ) -> Result<(Value, u64), String> {
        let (ns, path) = Self::profile_address(route);
        let descriptor = self
            .settings
            .describe(dsh_settings::SettingsDescribeOptions {
                redact_secrets: false,
            })
            .into_iter()
            .find(|view| view.ns.as_str() == ns)
            .ok_or("模型设置不可用")?;
        let value = descriptor.value.to_json().ok_or("模型设置不可用")?;
        let mut profile = if path.is_empty() {
            value
        } else {
            value
                .get("providers")
                .and_then(|p| p.get(route))
                .cloned()
                .or_else(|| create.then(|| json!({"models":[]})))
                .ok_or("供应商尚未配置")?
        };
        // Schema resolution materializes optional collection defaults. Those
        // empty maps are not user overrides and must not erase remote metadata.
        let raw = descriptor.user.as_ref().and_then(|v| v.to_json());
        let base = descriptor.base.as_ref().and_then(|v| v.to_json());
        for field in ["models", "modelPreferences"] {
            let user_field = raw
                .as_ref()
                .and_then(|v| {
                    if path.is_empty() {
                        Some(v)
                    } else {
                        v.get("providers").and_then(|p| p.get(route))
                    }
                })
                .and_then(|p| p.get(field));
            let base_field = base
                .as_ref()
                .and_then(|v| {
                    if path.is_empty() {
                        Some(v)
                    } else {
                        v.get("providers").and_then(|p| p.get(route))
                    }
                })
                .and_then(|p| p.get(field));
            if let Some(value) = user_field.or(base_field) {
                profile[field] = value.clone();
            } else if field == "modelPreferences" {
                profile
                    .as_object_mut()
                    .ok_or("模型设置不可用")?
                    .remove(field);
            }
        }
        if route == "deepseek-official" {
            profile["api"] = json!("openai-completions");
        }
        Ok((profile, descriptor.revision))
    }
    pub(super) async fn write_model_profile(
        &self,
        route: &str,
        profile: Value,
        revision: u64,
    ) -> Result<(), String> {
        let (namespace, path) = Self::profile_address(route);
        let ns = dsh_settings::settings_namespace(&namespace).map_err(|e| e.to_string())?;
        if path.is_empty() {
            let mut patch = profile;
            patch.as_object_mut().ok_or("模型配置无效")?.remove("api");
            self.settings.update(&ns, patch, Some(revision)).await
        } else {
            self.settings
                .update(&ns, json!({"providers":{route:profile}}), Some(revision))
                .await
        }
    }
    pub(crate) async fn ensure_catalog_scope(&self, route: &str) -> Result<String, String> {
        let profile = self.model_profile(route)?;
        let scope = if let Some(auth) = profile.get("authProvider").and_then(Value::as_str) {
            match self.session(auth).await? {
                Some(session) if !session.invalid => session.account_scope,
                None | Some(_) => "signed-out".into(),
            }
        } else if profile.get("keyless") == Some(&Value::Bool(true)) {
            format!(
                "anonymous-{}",
                crate::provider_auth_catalog::key(
                    profile.get("baseURL").and_then(Value::as_str).unwrap_or("")
                )
            )
        } else {
            let key = profile
                .get("apiKeyEnv")
                .and_then(Value::as_str)
                .and_then(|r| (!r.to_ascii_uppercase().starts_with("DSH_OAUTH_")).then_some(r));
            if let Some(reference) = key {
                self.credentials
                    .resolve(&dsh_credentials::credential_ref(reference))
                    .await
                    .map(|secret| {
                        format!(
                            "key-{}",
                            crate::provider_auth_catalog::key(&format!(
                                "{}\0{}",
                                profile.get("baseURL").and_then(Value::as_str).unwrap_or(""),
                                secret.value
                            ))
                        )
                    })
                    .unwrap_or_else(|| "unconfigured".into())
            } else {
                "unconfigured".into()
            }
        };
        self.catalogs.bind(route, &scope);
        Ok(scope)
    }
    pub(crate) async fn restore_catalog_bindings(&self) {
        let mut routes = vec!["deepseek-official".to_string()];
        if let Ok(ns) = dsh_settings::settings_namespace("llm-pi-ai")
            && let Some(value) = self.settings.get(&ns).and_then(|v| v.to_json())
            && let Some(providers) = value.get("providers").and_then(Value::as_object)
        {
            routes.extend(providers.keys().cloned());
        }
        for route in routes {
            let _ = self.ensure_catalog_scope(&route).await;
        }
    }
    pub(crate) fn effective_model_rows(&self, route: &str, profile: &Value) -> Vec<Value> {
        let scope = self
            .catalogs
            .scope(route)
            .unwrap_or_else(|| "unconfigured".into());
        let profile = self
            .model_profile(route)
            .unwrap_or_else(|_| profile.clone());
        merge_models(
            &profile,
            &self.catalogs.get(route, &scope),
            route == "deepseek-official",
        )
    }
    pub(crate) fn effective_native_config(&self, value: &Value) -> Value {
        let mut profile = value.clone();
        let rows=self.effective_model_rows("deepseek-official",value).into_iter().map(|mut row|{
            if let Some(input)=row.get("input").and_then(Value::as_array){row["imageInput"]=json!(input.iter().any(|v|v=="image"));}
            if let Some(levels)=row.get("reasoningEfforts").and_then(Value::as_object).cloned(){
                let descriptions=row.get("effortDescriptions").cloned().unwrap_or(json!({}));
                row["reasoningEfforts"]=Value::Array(levels.iter().map(|(id,wire)|json!({"id":id,"name":id,"wire":wire.as_str().unwrap_or("off"),"description":descriptions.get(id)})).collect());
            }else if row.get("reasoningEfforts")==Some(&Value::Bool(false)){row["reasoningEfforts"]=json!([]);}
            row
        }).collect::<Vec<_>>();
        profile["models"] = json!(rows);
        profile
    }
    pub(crate) async fn model_view(&self, route: &str) -> Result<Value, String> {
        let _guard = self.refresh.lock().await;
        let scope = self.ensure_catalog_scope(route).await?;
        let (profile, revision) = self.profile_snapshot(route, false)?;
        let catalog = self.catalogs.get(route, &scope);
        let (ns, path) = Self::profile_address(route);
        let mut preference_path = path.clone();
        preference_path.extend(["modelPreferences".into(), scope.clone()]);
        let models = merge_models(&profile, &catalog, route == "deepseek-official");
        Ok(
            json!({"provider":route,"settingsNs":ns,"settingsPath":path,"preferencePath":preference_path,
            "accountScope":scope,"models":models,"catalog":catalog.status_value(),
            "namespaceRevision":revision,"profileModels":profile.get("models").cloned().unwrap_or(json!([])),
            "preferences":profile.get("modelPreferences").and_then(|p|p.get(&scope)).cloned().unwrap_or(json!({}))}),
        )
    }
    pub(crate) async fn discovered_catalog(
        &self,
        route: &str,
    ) -> Result<Vec<dsh_llm::LlmDiscoveredModel>, String> {
        let view = self.refresh_catalog(route).await?;
        let scope = view["accountScope"].as_str().ok_or("目录账号状态无效")?;
        Ok(self.catalogs.get(route, scope).models)
    }
    async fn fetch_catalog_payload(&self, request: CatalogRequest) -> Result<Value, String> {
        let transport = self.catalog_transport.read().clone();
        if let Some(transport) = transport {
            return transport(request).await;
        }
        let mut response = self
            .client
            .get(request.url)
            .headers(request.headers)
            .send()
            .await
            .map_err(|_| "模型目录服务暂时无法连接".to_string())?;
        if !response.status().is_success() {
            return Err(format!("模型目录返回HTTP {}", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "读取模型目录失败")? {
            if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
                return Err("模型目录响应超过8 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "模型目录返回无效JSON".into())
    }
    pub(crate) async fn refresh_catalog(&self, route: &str) -> Result<Value, String> {
        if route == "opencode-free" {
            return Err(
                "请通过免费模型目录刷新候选，普通供应商目录不会导入未经核验的免费模型".into(),
            );
        }
        let lock = self
            .catalogs
            .locks
            .lock()
            .entry(route.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _sync = lock.lock().await;
        let profile = self.model_profile(route)?;
        let key = if let Some(auth) = profile.get("authProvider").and_then(Value::as_str) {
            self.resolve_token(auth).await?
        } else if profile.get("keyless") == Some(&Value::Bool(true)) {
            None
        } else if let Some(reference) = profile.get("apiKeyEnv").and_then(Value::as_str) {
            super::super::deepseek_settings::validate_api_key_reference(reference)?;
            self.credentials
                .resolve(&dsh_credentials::credential_ref(reference))
                .await
                .map(|v| v.value)
        } else {
            None
        };
        let profile = self.model_profile(route)?;
        let scope = self.ensure_catalog_scope(route).await?;
        if scope == "signed-out" {
            return Err("账号尚未登录".into());
        }
        self.catalogs.mark_syncing(route, &scope);
        let result = async {
            let mut url = catalog_url(&profile)?;
            let mut headers = super::super::validated_discovery_headers(
                route,
                &serde_json::from_value(profile.get("headers").cloned().unwrap_or(json!({})))
                    .map_err(|_| "供应商请求头配置无效")?,
            )?;
            headers.remove(reqwest::header::AUTHORIZATION);
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            );
            if let Some(key) = &key {
                let official_anthropic = url.host_str() == Some("api.anthropic.com")
                    && profile
                        .get("authProvider")
                        .and_then(Value::as_str)
                        .is_none();
                let header = if official_anthropic {
                    "x-api-key"
                } else {
                    "authorization"
                };
                let raw = if official_anthropic {
                    key.clone()
                } else {
                    format!("Bearer {key}")
                };
                headers.insert(
                    reqwest::header::HeaderName::from_static(header),
                    reqwest::header::HeaderValue::from_str(&raw)
                        .map_err(|_| "凭据不是有效HTTP值")?,
                );
            }
            if profile.get("authProvider") == Some(&json!("openai-codex")) {
                let account = self
                    .session("openai-codex")
                    .await?
                    .and_then(|s| s.account_id)
                    .ok_or("账号授权未提供账号标识，请重新登录后刷新目录")?;
                headers.insert(
                    "chatgpt-account-id",
                    reqwest::header::HeaderValue::from_str(&account).map_err(|_| "账号标识无效")?,
                );
            }
            if profile.get("api") == Some(&json!("anthropic-messages")) {
                headers.insert(
                    "anthropic-version",
                    reqwest::header::HeaderValue::from_static("2023-06-01"),
                );
            }
            let source = url.to_string();
            let mut models = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for page in 0..20 {
                let payload = self
                    .fetch_catalog_payload(CatalogRequest {
                        url: url.clone(),
                        headers: headers.clone(),
                    })
                    .await?;
                for model in super::super::model_discovery::parse_model_listing(&payload)? {
                    if seen.insert(model.id.clone()) {
                        models.push(model);
                    }
                }
                if models.len() > 10_000 {
                    return Err("模型目录条目过多".to_string());
                }
                if payload.get("has_more") == Some(&Value::Bool(true)) {
                    let cursor = payload
                        .get("last_id")
                        .and_then(Value::as_str)
                        .ok_or("目录缺少下一页游标")?;
                    if page == 19 {
                        return Err("模型目录分页超过限制".into());
                    }
                    let query = url
                        .query_pairs()
                        .filter(|(name, _)| name != "after_id")
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect::<Vec<_>>();
                    url.query_pairs_mut()
                        .clear()
                        .extend_pairs(query)
                        .append_pair("after_id", cursor);
                } else {
                    break;
                }
            }
            if models.is_empty() {
                return Err(
                    "账号目录未返回可用模型；请检查账号权限或稍后刷新，未使用固定模型替代"
                        .to_string(),
                );
            }
            Ok::<_, String>((models, source))
        }
        .await;
        match result {
            Ok((models, source)) => {
                let commit_guard = self.refresh.lock().await;
                // Auth changes while a request is in flight cannot publish
                // another account's response as the current directory.
                if self.ensure_catalog_scope(route).await? != scope {
                    return Err("账号已切换，请刷新当前账号目录".into());
                }
                self.catalogs
                    .store(Catalog {
                        provider: route.into(),
                        account_scope: scope.clone(),
                        status: "synced".into(),
                        source: "remote".into(),
                        endpoint: Some(source),
                        updated_at: Some(now()),
                        error: None,
                        models,
                    })
                    .await?;
                let mut committed = false;
                for attempt in 0..4 {
                    let (mut latest, revision) = self.profile_snapshot(route, false)?;
                    migrate_legacy_preferences(&mut latest, &scope);
                    latest["modelCatalogScope"] = json!(scope);
                    latest["catalogRevision"] = json!(uuid::Uuid::new_v4().to_string());
                    match self.write_model_profile(route, latest, revision).await {
                        Ok(()) => {
                            committed = true;
                            break;
                        }
                        Err(error)
                            if attempt < 3
                                && (error.contains("revision")
                                    || error.to_lowercase().contains("conflict")) =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                if !committed {
                    return Err("模型设置持续变化，请稍后刷新".into());
                }
                drop(commit_guard);
                self.model_view(route).await
            }
            Err(error) => {
                self.catalogs.fail(route, &scope, &error).await;
                Err(error)
            }
        }
    }
}
