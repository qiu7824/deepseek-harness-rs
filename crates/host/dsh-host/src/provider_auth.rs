//! Account authorization and renewal. Tokens stay in the credential provider;
//! the browser receives a user code, an opaque attempt id, and account status.
#[cfg(test)]
#[path = "provider_auth_models_tests.rs"]
mod model_tests;
#[path = "provider_auth_models.rs"]
mod models;
#[cfg(test)]
#[path = "provider_auth_review_tests.rs"]
mod review_tests;
use dsh_credentials::CredentialProvider;
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
struct Provider {
    id: &'static str,
    name: &'static str,
    issuer: &'static str,
    client: &'static str,
    device: &'static str,
    token: &'static str,
    scope: &'static str,
    base: &'static str,
    api: &'static str,
}
const PROVIDERS: &[Provider] = &[
    Provider {
        id: "copilot",
        name: "GitHub Copilot",
        issuer: "https://github.com",
        client: "Iv1.b507a08c87ecfe98",
        device: "https://github.com/login/device/code",
        token: "https://github.com/login/oauth/access_token",
        scope: "read:user",
        base: "https://api.githubcopilot.com",
        api: "openai-completions",
    },
    Provider {
        id: "qwen-oauth",
        name: "Qwen",
        issuer: "https://chat.qwen.ai",
        client: "f0304373b74a44d2b584a3fb70ca9e56",
        device: "https://chat.qwen.ai/api/v1/oauth2/device/code",
        token: "https://chat.qwen.ai/api/v1/oauth2/token",
        scope: "openid profile email model.completion",
        base: "https://portal.qwen.ai/v1",
        api: "openai-completions",
    },
    Provider {
        id: "minimax-oauth",
        name: "MiniMax International",
        issuer: "https://api.minimax.io",
        client: "78257093-7e40-4613-99e0-527b14b39113",
        device: "https://account.minimax.io/oauth2/device/code",
        token: "https://account.minimax.io/oauth2/token",
        scope: "group_id profile model.completion",
        base: "https://api.minimax.io/anthropic",
        api: "anthropic-messages",
    },
    Provider {
        id: "minimax-cn-oauth",
        name: "MiniMax 中国",
        issuer: "https://api.minimaxi.com",
        client: "78257093-7e40-4613-99e0-527b14b39113",
        device: "https://account.minimaxi.com/oauth2/device/code",
        token: "https://account.minimaxi.com/oauth2/token",
        scope: "group_id profile model.completion",
        base: "https://api.minimaxi.com/anthropic",
        api: "anthropic-messages",
    },
    Provider {
        id: "nous",
        name: "Nous Portal",
        issuer: "https://portal.nousresearch.com",
        client: "hermes-cli",
        device: "https://portal.nousresearch.com/api/oauth/device/code",
        token: "https://portal.nousresearch.com/api/oauth/token",
        scope: "inference:invoke",
        base: "https://inference-api.nousresearch.com/v1",
        api: "openai-completions",
    },
    Provider {
        id: "openai-codex",
        name: "ChatGPT / Codex",
        issuer: "https://auth.openai.com",
        client: "app_EMoamEEZ73f0CkXaXp7hrann",
        device: "https://auth.openai.com/api/accounts/deviceauth/usercode",
        token: "https://auth.openai.com/oauth/token",
        scope: "",
        base: "https://chatgpt.com/backend-api/codex",
        api: "openai-responses",
    },
    Provider {
        id: "xai-oauth",
        name: "xAI Grok",
        issuer: "https://auth.x.ai",
        client: "b1a00492-073a-47ea-816f-4c329264a828",
        device: "https://auth.x.ai/oauth2/device/code",
        token: "https://auth.x.ai/oauth2/token",
        scope: "openid profile email offline_access grok-cli:access api:access",
        base: "https://api.x.ai/v1",
        api: "openai-responses",
    },
];
fn provider(id: &str) -> Result<Provider, String> {
    PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .copied()
        .ok_or_else(|| "不支持的账号提供商".to_string())
}
pub(crate) fn valid_profile(id: &str, base: &str, api: &str) -> bool {
    provider(id).is_ok_and(|p| {
        (p.base == base.trim_end_matches('/') || id == "copilot" && trusted_copilot_base(base))
            && p.api == api
    })
}
fn trusted_copilot_base(base: &str) -> bool {
    reqwest::Url::parse(base).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.host_str().is_some_and(|host| {
                host == "api.githubcopilot.com" || host.ends_with(".githubcopilot.com")
            })
    })
}
fn pkce() -> (String, String) {
    use base64::Engine;
    use sha2::Digest;
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}
fn expiry(value: &Value) -> u64 {
    let raw = number(value, "expired_in", 0);
    if raw > now() * 500 {
        raw / 1000
    } else {
        now()
            + if raw > 0 {
                raw
            } else {
                number(value, "expires_in", 3600)
            }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn reference(id: &str) -> dsh_credentials::CredentialRef {
    dsh_credentials::credential_ref(&format!(
        "DSH_OAUTH_{}_SESSION",
        id.replace('-', "_").to_ascii_uppercase()
    ))
}
fn accounts_reference(id: &str) -> dsh_credentials::CredentialRef {
    dsh_credentials::credential_ref(&format!(
        "DSH_OAUTH_{}_ACCOUNTS",
        id.replace('-', "_").to_ascii_uppercase()
    ))
}
fn string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("登录服务未返回 {key}"))
}
fn number(value: &Value, key: &str, default: u64) -> u64 {
    value
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(default)
}
fn jwt(token: &str) -> Option<Value> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1)?)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}
#[derive(Clone, Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    invalid: bool,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    account_scope: String,
}

fn codex_refresh_requires_login(status: u16, value: &Value) -> bool {
    let code = value
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/code").and_then(Value::as_str));
    matches!(status, 400 | 401)
        && matches!(
            code,
            Some(
                "invalid_grant"
                    | "refresh_token_expired"
                    | "refresh_token_reused"
                    | "refresh_token_invalidated"
                    | "refresh_token_revoked"
            )
        )
}
impl Session {
    fn identity_scope(account_id: Option<&str>, claims: Option<&Value>) -> Option<String> {
        let subject = claims.and_then(|claims| {
            claims.get("sub").and_then(Value::as_str).filter(|value| !value.is_empty()).or_else(|| {
                claims.get("https://api.openai.com/auth")?.get("chatgpt_user_id")?.as_str()
            })
        }).filter(|value| !value.is_empty());
        match (account_id, subject) {
            (Some(account), Some(subject)) => Some(format!("account-v2-{}", crate::provider_auth_catalog::key(&json!([account, subject]).to_string()))),
            (Some(identity), None) | (None, Some(identity)) => Some(format!("account-{}", crate::provider_auth_catalog::key(identity))),
            _ => None,
        }
    }
    fn normalize_identity(&mut self) {
        let claims = jwt(&self.access_token);
        if let Some(account) = claims.as_ref().and_then(|v| v.get("https://api.openai.com/auth"))
            .and_then(|v| v.get("chatgpt_account_id")).and_then(Value::as_str) {
            if self.account_id.as_deref() != Some(account) { self.account_scope.clear(); }
            self.account_id = Some(account.into());
        }
        if let Some(scope) = Self::identity_scope(self.account_id.as_deref(), claims.as_ref()) {
            // An opaque refreshed access token must retain the complete identity
            // established by its previous JWT, not fall back to workspace-only.
            if scope.starts_with("account-v2-") || !self.account_scope.starts_with("account-v2-") {
                self.account_scope = scope;
            }
        } else if self.account_scope.is_empty() {
            self.account_scope = format!("account-{}", crate::provider_auth_catalog::key(self.refresh_token.as_deref().unwrap_or(&self.access_token)));
        }
    }
    fn from_tokens(value: &Value, previous: Option<&Session>) -> Result<Self, String> {
        let access_token = string(value, "access_token")?;
        let claims = jwt(&access_token);
        let account_id = claims
            .as_ref()
            .and_then(|c| c.get("https://api.openai.com/auth"))
            .and_then(|c| c.get("chatgpt_account_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| previous.and_then(|p| p.account_id.clone()));
        let claimed_scope = Self::identity_scope(account_id.as_deref(), claims.as_ref());
        let account_scope = previous.filter(|p| {
                p.account_id == account_id && (claimed_scope.is_none()
                    || p.account_scope.starts_with("account-v2-")
                        && !claimed_scope.as_deref().is_some_and(|scope| scope.starts_with("account-v2-")))
            }).map(|p| p.account_scope.clone())
            .or(claimed_scope)
            .or_else(|| {
                previous
                    .filter(|p| !p.account_scope.is_empty())
                    .map(|p| p.account_scope.clone())
            })
            .unwrap_or_else(|| format!("account-{}", uuid::Uuid::new_v4().simple()));
        Ok(Self {
            expires_at: claims
                .as_ref()
                .and_then(|c| c.get("exp"))
                .and_then(Value::as_u64)
                .unwrap_or_else(|| expiry(value)),
            access_token,
            account_id,
            account_scope,
            invalid: false,
            base_url: previous.and_then(|p| p.base_url.clone()),
            refresh_token: value
                .get("refresh_token")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
        })
    }
}
struct Pending {
    provider: Provider,
    device: String,
    user_code: String,
    expires: u64,
    next_poll: u64,
    interval: u64,
    verifier: Option<String>,
}
pub(crate) struct AccountAuth {
    account_usage: crate::codex_account::CodexAccountService,
    client: reqwest::Client,
    credentials: Arc<dsh_credentials_local::LocalCredentialProvider>,
    settings: Arc<dsh_settings::SettingsProvider>,
    pending: parking_lot::Mutex<HashMap<String, (Provider, Arc<tokio::sync::Mutex<Pending>>)>>,
    refresh: tokio::sync::Mutex<()>,
    cli: parking_lot::RwLock<Option<Arc<super::claude_cli_auth::ClaudeCliAuth>>>,
    catalogs: crate::provider_auth_catalog::CatalogStore,
    catalog_transport: parking_lot::RwLock<Option<models::CatalogTransport>>,
}
impl AccountAuth {
    pub(crate) fn register_usage_tool(
        self: &Arc<Self>,
        ctx: &cordis::Context,
    ) -> Result<(), String> {
        use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .map(|s| s.as_ref().clone())
            .ok_or("账户用量工具缺少工具运行时")?;
        let auth = self.clone();
        tools.register(ctx,ToolDefinition{
            name:"codex_usage".into(),
            description:"Read the verified Codex account's shared usage limits, reset times, and available reset-card count. This tool is read-only and never redeems a card. Unknown metrics remain null; account usage is distinct from this task's token usage.".into(),
            parameters:json!({"type":"object","properties":{"refresh":{"type":"boolean"}},"additionalProperties":false}),
            output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
            timeout_ms:Some(45000),is_concurrency_safe:None,
            finalize_content:None,present_call:None,present_result:None,
            execute:Arc::new(move |args,_|{let auth=auth.clone();let refresh=args.get("refresh").and_then(Value::as_bool).unwrap_or(false);Box::pin(async move{auth.handle("usage",&json!({"provider":"openai-codex","refresh":refresh})).await.map_err(ToolBodyError::plain)})}),
        }).map(|_|())
    }
    pub(crate) fn new(
        credentials: Arc<dsh_credentials_local::LocalCredentialProvider>,
        settings: Arc<dsh_settings::SettingsProvider>,
    ) -> Result<Arc<Self>, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(25))
            .user_agent("deepseek-harness-rs")
            .default_headers(reqwest::header::HeaderMap::from_iter([(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            )]))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            account_usage: crate::codex_account::CodexAccountService::new(
                std::path::Path::new(credentials.filename())
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("environments/codex-account"),
            ),
            client,
            catalogs: crate::provider_auth_catalog::CatalogStore::new(
                std::path::Path::new(credentials.filename())
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("cache/model-catalogs"),
            ),
            catalog_transport: Default::default(),
            credentials,
            settings,
            pending: Default::default(),
            refresh: tokio::sync::Mutex::new(()),
            cli: Default::default(),
        }))
    }
    pub(crate) fn set_claude_cli(&self, cli: Arc<super::claude_cli_auth::ClaudeCliAuth>) {
        *self.cli.write() = Some(cli);
    }
    async fn session(&self, id: &str) -> Result<Option<Session>, String> {
        let loaded = self.credentials
            .resolve(&reference(id))
            .await
            .map(|value| {
                let mut session: Session = serde_json::from_str(&value.value)
                    .map_err(|_| "账号凭据无效，请重新登录".to_string())?;
                let old_scope = session.account_scope.clone();
                session.normalize_identity();
                Ok::<_, String>((session, old_scope))
            })
            .transpose()?;
        let Some((session, old_scope)) = loaded else { return Ok(None); };
        self.migrate_session_catalog(id, &session, &old_scope).await;
        Ok(Some(session))
    }
    async fn migrate_session_catalog(&self, id: &str, session: &Session, old_scope: &str) {
        if let Some(account) = session.account_id.as_deref().filter(|_| session.account_scope.starts_with("account-v2-")) {
            let legacy = format!("account-{}", crate::provider_auth_catalog::key(account));
            if old_scope.is_empty() || old_scope == legacy {
                self.catalogs.migrate_login_scope(id, &legacy, &session.account_scope).await;
            }
        }
    }
    async fn saved_sessions(&self, id: &str) -> Result<Vec<Session>, String> {
        let Some(value) = self.credentials.resolve(&accounts_reference(id)).await else {
            return Ok(Vec::new());
        };
        let accounts: Vec<Session> = serde_json::from_str(&value.value).map_err(|_| "账号目录凭据无效，请重新登录".to_string())?;
        let mut normalized: Vec<Session> = Vec::new();
        for mut account in accounts {
            let old_scope = account.account_scope.clone();
            account.normalize_identity();
            self.migrate_session_catalog(id, &account, &old_scope).await;
            normalized.retain(|saved| saved.account_scope != account.account_scope);
            normalized.push(account);
        }
        Ok(normalized)
    }
    // Callers hold `refresh` while changing accounts. Include the legacy active
    // credential before replacing it, so the first additional login preserves it.
    async fn account_sessions(&self, id: &str) -> Result<Vec<Session>, String> {
        let mut accounts = self.saved_sessions(id).await?;
        if let Some(current) = self.session(id).await? {
            if let Some(saved) = accounts
                .iter_mut()
                .find(|item| item.account_scope == current.account_scope)
            {
                *saved = current;
            } else {
                accounts.push(current);
            }
        }
        Ok(accounts)
    }
    async fn write_accounts(&self, id: &str, accounts: &[Session]) -> Result<(), String> {
        if accounts.is_empty() {
            self.credentials.unset(&accounts_reference(id)).await
        } else {
            self.credentials
                .set(
                    &accounts_reference(id),
                    &serde_json::to_string(accounts).map_err(|e| e.to_string())?,
                )
                .await
        }
    }
    async fn save(&self, id: &str, session: &Session) -> Result<(), String> {
        let mut accounts = self.account_sessions(id).await?;
        accounts.retain(|item| item.account_scope != session.account_scope);
        accounts.push(session.clone());
        self.write_accounts(id, &accounts).await?;
        self.credentials
            .set(
                &reference(id),
                &serde_json::to_string(session).map_err(|e| e.to_string())?,
            )
            .await
    }
    async fn activate(&self, p: Provider, session: &Session) -> Result<(), String> {
        let previous = self.session(p.id).await?;
        self.save(p.id, session).await?;
        if let Err(error) = self.install_profile_with_previous(p, session, previous.as_ref()).await {
            // Keep the newly authorized account in the directory, but restore
            // the active credential when its route cannot be installed.
            let restored = match previous.as_ref() {
                Some(previous) => {
                    self.catalogs.bind(p.id, &previous.account_scope);
                    self.credentials
                        .set(
                            &reference(p.id),
                            &serde_json::to_string(previous).map_err(|e| e.to_string())?,
                        )
                        .await
                }
                None => {
                    self.catalogs.unbind(p.id);
                    self.credentials.unset(&reference(p.id)).await
                }
            };
            return Err(match restored {
                Ok(()) => format!("账号配置未能保存，活动账号保持不变：{error}"),
                Err(restore_error) => format!(
                    "账号配置未能保存且凭据恢复失败，请重新连接账号：{error}；{restore_error}"
                ),
            });
        }
        Ok(())
    }
    pub(crate) async fn resolve_token(&self, id: &str) -> Result<Option<String>, String> {
        let _guard = self.refresh.lock().await;
        self.resolve_token_locked(id, false, true).await
    }
    /// The caller retains the refresh lock, including any account RPC refresh callback.
    async fn resolve_token_locked(
        &self,
        id: &str,
        force: bool,
        update_profile: bool,
    ) -> Result<Option<String>, String> {
        let p = provider(id)?;
        let Some(mut session) = self.session(id).await? else {
            return Err("账号尚未登录，请在模型设置中登录".to_string());
        };
        if session.invalid {
            return Err("账号授权已失效，请重新登录".to_string());
        }
        if !force && session.expires_at > now() + 120 {
            if update_profile {
                self.repair_profile(p, &session).await?;
            }
            return Ok(Some(session.access_token));
        }
        let refresh = session
            .refresh_token
            .as_deref()
            .ok_or_else(|| "账号授权已过期，请重新登录".to_string())?;
        if id == "copilot" {
            let renewed = self.exchange_copilot(refresh).await?;
            self.save(id, &renewed).await?;
            if update_profile {
                self.repair_profile(p, &renewed).await?;
            }
            return Ok(Some(renewed.access_token));
        }
        let request = self.client.post(p.token);
        let request = request.form(&[
            ("grant_type", "refresh_token"),
            ("client_id", p.client),
            ("refresh_token", refresh),
        ]);
        let response = request
            .send()
            .await
            .map_err(|_| "账号刷新服务暂时无法连接".to_string())?;
        let status = response.status();
        let tokens = response
            .json::<Value>()
            .await
            .map_err(|_| "账号刷新服务返回无效数据".to_string())?;
        if !status.is_success() {
            let invalid = if id == "openai-codex" {
                codex_refresh_requires_login(status.as_u16(), &tokens)
            } else {
                matches!(status.as_u16(), 400 | 401 | 403)
            };
            if invalid {
                session.invalid = true;
                session.refresh_token = None;
                session.access_token.clear();
                self.save(id, &session).await?;
            }
            return Err(if invalid {
                format!("账号刷新凭据已失效（HTTP {}），请重新登录", status.as_u16())
            } else {
                format!(
                    "账号刷新暂未完成（HTTP {}），已保留登录状态，请稍后重试",
                    status.as_u16()
                )
            });
        }
        session = Session::from_tokens(&tokens, Some(&session))?;
        self.save(id, &session).await?;
        if update_profile {
            self.repair_profile(p, &session).await?;
        }
        Ok(Some(session.access_token))
    }
    async fn repair_profile(&self, p: Provider, session: &Session) -> Result<(), String> {
        let current = self.profile_snapshot(p.id, true)?.0;
        let account_header = current
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("chatgpt-account-id"))
            })
            .and_then(|(_, value)| value.as_str());
        if current.get("baseURL").and_then(Value::as_str)
            != Some(session.base_url.as_deref().unwrap_or(p.base))
            || current.get("modelCatalogScope").and_then(Value::as_str)
                != Some(session.account_scope.as_str())
            || p.id == "openai-codex" && account_header != session.account_id.as_deref()
        {
            self.install_profile(p, session).await?;
        }
        Ok(())
    }
    pub(crate) async fn resolve_request_token_for_scope(
        &self,
        id: &str,
        base: &str,
        headers: &[(String, String)],
        expected_scope: Option<&str>,
    ) -> Result<Option<String>, String> {
        let _guard = self.refresh.lock().await;
        // Compare the captured route before repair: a legacy active profile may
        // legitimately be migrated by resolve_token_locked below, but a route
        // captured before a switch must never use the newly active credential.
        let before = self.profile_snapshot(id, false)?.0;
        if expected_scope.is_some_and(|scope| before.get("modelCatalogScope").and_then(Value::as_str) != Some(scope)) {
            return Err("账号请求配置已更新，请重试当前请求".into());
        }
        let selected_scope = self.session(id).await?.map(|session| session.account_scope);
        let token = self.resolve_token_locked(id, false, true).await?;
        let current = self.profile_snapshot(id, false)?.0;
        let header_changed = current
            .get("headers")
            .and_then(Value::as_object)
            .is_some_and(|current| {
                current.iter().any(|(name, value)| {
                    !headers.iter().any(|(old, old_value)| {
                        old.eq_ignore_ascii_case(name) && value.as_str() == Some(old_value)
                    })
                })
            });
        if current.get("baseURL").and_then(Value::as_str) != Some(base) || header_changed
            || selected_scope.as_deref().is_some_and(|scope| current.get("modelCatalogScope").and_then(Value::as_str) != Some(scope)) {
            return Err("账号请求配置已更新，请重试当前请求".into());
        }
        Ok(token)
    }
    async fn start(&self, id: &str) -> Result<Value, String> {
        let p = provider(id)?;
        self.pending.lock().retain(|_, (_, pending)| {
            pending
                .try_lock()
                .map(|p| p.expires > now())
                .unwrap_or(true)
        });
        if self.pending.lock().len() >= 8 {
            return Err("登录请求过多，请取消未完成的登录".to_string());
        }
        let request = self.client.post(p.device);
        let (verifier, challenge) = pkce();
        let state = uuid::Uuid::new_v4().to_string();
        let minimax = id.starts_with("minimax");
        let response = if p.id == "openai-codex" {
            request.json(&json!({"client_id":p.client})).send().await
        } else if minimax {
            request
                .form(&[
                    ("response_type", "code"),
                    ("client_id", p.client),
                    ("scope", p.scope),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                    ("state", &state),
                ])
                .send()
                .await
        } else if id == "qwen-oauth" {
            request
                .form(&[
                    ("client_id", p.client),
                    ("scope", p.scope),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                ])
                .send()
                .await
        } else {
            request
                .form(&[("client_id", p.client), ("scope", p.scope)])
                .send()
                .await
        }
        .map_err(|_| "无法连接账号登录服务，请检查网络后重试".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "账号登录服务返回 HTTP {}",
                response.status().as_u16()
            ));
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| "账号登录服务返回无效数据".to_string())?;
        if minimax && value.get("state").and_then(Value::as_str) != Some(&state) {
            return Err("登录状态校验失败，请重新登录".to_string());
        }
        let user_code = string(&value, "user_code")?;
        let device = if minimax {
            user_code.clone()
        } else {
            string(
                &value,
                if p.id == "openai-codex" {
                    "device_auth_id"
                } else {
                    "device_code"
                },
            )?
        };
        let verification = if p.id == "openai-codex" {
            "https://auth.openai.com/codex/device".to_string()
        } else {
            value
                .get("verification_uri_complete")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(string(&value, "verification_uri")?)
        };
        let url = reqwest::Url::parse(&verification).map_err(|_| "登录验证链接无效".to_string())?;
        let issuer = reqwest::Url::parse(p.issuer).expect("static issuer");
        let verification_host = url.host_str().unwrap_or("");
        let trusted_host = url.host_str() == issuer.host_str()
            || id == "xai-oauth" && verification_host == "accounts.x.ai"
            || minimax
                && ["minimax.io", "minimaxi.com"].iter().any(|domain| {
                    verification_host == *domain
                        || verification_host.ends_with(&format!(".{domain}"))
                })
            || id == "qwen-oauth"
                && (verification_host == "qwen.ai" || verification_host.ends_with(".qwen.ai"));
        if url.scheme() != "https"
            || !trusted_host
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("登录服务返回了不受信任的验证地址".to_string());
        }
        let expires = if minimax {
            expiry(&value).min(now() + 1800)
        } else {
            now() + number(&value, "expires_in", 900).clamp(30, 1800)
        };
        let interval = if minimax {
            (number(&value, "interval", 2000) / 1000).clamp(2, 60)
        } else {
            number(&value, "interval", 5).clamp(3, 60)
        };
        let attempt = uuid::Uuid::new_v4().to_string();
        self.pending.lock().insert(
            attempt.clone(),
            (
                p,
                Arc::new(tokio::sync::Mutex::new(Pending {
                    provider: p,
                    device,
                    user_code: user_code.clone(),
                    expires,
                    next_poll: now() + interval,
                    interval,
                    verifier: (minimax || id == "qwen-oauth").then_some(verifier),
                })),
            ),
        );
        Ok(
            json!({"attempt":attempt,"userCode":user_code,"verificationUri":verification,"expiresAt":expires,"interval":interval}),
        )
    }
    async fn poll(&self, attempt: &str) -> Result<Value, String> {
        let pending = self
            .pending
            .lock()
            .get(attempt)
            .map(|(_, pending)| pending.clone())
            .ok_or_else(|| "登录请求已结束，请重新开始".to_string())?;
        let mut pending = pending.lock().await;
        if now() >= pending.expires {
            self.pending.lock().remove(attempt);
            return Err("登录已超时，请重新开始".to_string());
        }
        if now() < pending.next_poll {
            return Ok(json!({"status":"pending","interval":pending.interval}));
        }
        pending.next_poll = now() + pending.interval;
        let p = pending.provider;
        let response = if p.id == "openai-codex" {
            self.client
                .post("https://auth.openai.com/api/accounts/deviceauth/token")
                .json(&json!({"device_auth_id":pending.device,"user_code":pending.user_code}))
                .send()
                .await
        } else if p.id.starts_with("minimax") {
            self.client
                .post(p.token)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:user_code"),
                    ("client_id", p.client),
                    ("user_code", pending.user_code.as_str()),
                    ("code_verifier", pending.verifier.as_deref().unwrap_or("")),
                ])
                .send()
                .await
        } else if p.id == "qwen-oauth" {
            self.client
                .post(p.token)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", p.client),
                    ("device_code", pending.device.as_str()),
                    ("code_verifier", pending.verifier.as_deref().unwrap_or("")),
                ])
                .send()
                .await
        } else {
            self.client
                .post(p.token)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", p.client),
                    ("device_code", &pending.device),
                ])
                .send()
                .await
        }
        .map_err(|_| "登录状态查询失败，请重试".to_string())?;
        let status = response.status();
        if p.id == "openai-codex" && matches!(status.as_u16(), 403 | 404) {
            return Ok(json!({"status":"pending","interval":pending.interval}));
        }
        let mut value = response
            .json::<Value>()
            .await
            .map_err(|_| "登录服务返回无效数据".to_string())?;
        if p.id.starts_with("minimax")
            && status.is_success()
            && value.get("status").and_then(Value::as_str) != Some("success")
        {
            if value.get("status").and_then(Value::as_str) == Some("error") {
                self.pending.lock().remove(attempt);
                return Err("MiniMax 拒绝了登录授权".to_string());
            }
            return Ok(json!({"status":"pending","interval":pending.interval}));
        }
        if !status.is_success() || value.get("error").is_some() {
            match value.get("error").and_then(Value::as_str) {
                Some("authorization_pending") => {
                    return Ok(json!({"status":"pending","interval":pending.interval}));
                }
                Some("slow_down") => {
                    pending.interval = (pending.interval + 5).min(60);
                    pending.next_poll = now() + pending.interval;
                    return Ok(json!({"status":"pending","interval":pending.interval}));
                }
                _ => {
                    self.pending.lock().remove(attempt);
                    return Err(format!(
                        "账号授权失败（HTTP {}），请重新登录",
                        status.as_u16()
                    ));
                }
            }
        }
        if p.id == "openai-codex" {
            let code = string(&value, "authorization_code")?;
            let verifier = string(&value, "code_verifier")?;
            let response = self
                .client
                .post(p.token)
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", p.client),
                    ("code", &code),
                    ("code_verifier", &verifier),
                    (
                        "redirect_uri",
                        "https://auth.openai.com/deviceauth/callback",
                    ),
                ])
                .send()
                .await
                .map_err(|_| "登录授权交换失败".to_string())?;
            if !response.status().is_success() {
                self.pending.lock().remove(attempt);
                return Err(format!(
                    "登录授权交换失败（HTTP {}）",
                    response.status().as_u16()
                ));
            }
            value = response
                .json()
                .await
                .map_err(|_| "登录授权交换返回无效数据".to_string())?;
        }
        // A cancelled attempt cannot commit credentials after an in-flight poll.
        if !self.pending.lock().contains_key(attempt) {
            return Err("登录已取消".to_string());
        }
        let session = if p.id == "copilot" {
            self.exchange_copilot(&string(&value, "access_token")?)
                .await?
        } else {
            Session::from_tokens(&value, None)?
        };
        self.commit(attempt, p, &session).await?;
        Ok(json!({"status":"complete","provider":p.id}))
    }
    async fn commit(&self, attempt: &str, p: Provider, session: &Session) -> Result<(), String> {
        {
            let _guard = self.refresh.lock().await;
            if !self.pending.lock().contains_key(attempt) {
                return Err("登录已取消".to_string());
            }
            self.activate(p, session).await?;
            self.pending.lock().remove(attempt);
        }
        // Authentication and catalog sync are separate states. An authenticated
        // account with a failing catalog retains an actionable sync error.
        let _ = self.refresh_catalog(p.id).await;
        Ok(())
    }
    async fn install_profile(&self, p: Provider, session: &Session) -> Result<(), String> {
        self.install_profile_with_previous(p, session, None).await
    }
    async fn install_profile_with_previous(&self, p: Provider, session: &Session, previous: Option<&Session>) -> Result<(), String> {
        for attempt in 0..4 {
            let (mut profile, revision) = self.profile_snapshot(p.id, true)?;
            let owner = previous.unwrap_or(session);
            if owner.account_scope.starts_with("account-v2-") {
                if let Some(account) = owner.account_id.as_deref() {
                    let legacy = format!("account-{}", crate::provider_auth_catalog::key(account));
                    if let Some(preferences) = profile.get_mut("modelPreferences").and_then(Value::as_object_mut) {
                        if let Some(old) = preferences.remove(&legacy) {
                            let current = preferences.entry(owner.account_scope.clone()).or_insert_with(|| json!({}));
                            if let (Some(current), Some(old)) = (current.as_object_mut(), old.as_object()) {
                                for (model, value) in old { current.entry(model.clone()).or_insert_with(|| value.clone()); }
                            }
                        }
                    }
                    if profile.get("legacyModelScope").and_then(Value::as_str) == Some(&legacy) { profile["legacyModelScope"] = json!(owner.account_scope); }
                    if let Some(models) = profile.get_mut("models").and_then(Value::as_array_mut) {
                        for model in models { if model.get("accountScope").and_then(Value::as_str) == Some(&legacy) { model["accountScope"] = json!(owner.account_scope); } }
                    }
                }
            }
            crate::provider_auth_catalog::migrate_legacy_preferences(
                &mut profile,
                &session.account_scope,
            );
            profile["authProvider"] = json!(p.id);
            profile["api"] = json!(p.api);
            profile["baseURL"] = json!(session.base_url.as_deref().unwrap_or(p.base));
            if profile.get("displayName").is_none() {
                profile["displayName"] = json!(p.name);
            }
            profile["apiKeyEnv"] = json!(reference(p.id).as_str());
            profile["modelCatalogScope"] = json!(session.account_scope);
            profile["catalogRevision"] = json!(uuid::Uuid::new_v4().to_string());
            profile["keyless"] = json!(false);
            let mut headers = profile
                .get("headers")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if p.id == "openai-codex" {
                headers.retain(|name, _| !name.eq_ignore_ascii_case("chatgpt-account-id"));
                if let Some(account) = &session.account_id {
                    headers.insert("ChatGPT-Account-ID".into(), json!(account));
                }
            }
            if p.id == "copilot" {
                for (name, value) in [
                    ("Editor-Version", "vscode/1.104.1"),
                    ("Copilot-Integration-Id", "vscode-chat"),
                    ("Openai-Intent", "conversation-edits"),
                    ("x-initiator", "agent"),
                ] {
                    headers.retain(|key, _| !key.eq_ignore_ascii_case(name));
                    headers.insert(name.into(), json!(value));
                }
            }
            profile["headers"] = Value::Object(headers);
            self.catalogs.bind(p.id, &session.account_scope);
            match self.write_model_profile(p.id, profile, revision).await {
                Ok(()) => return Ok(()),
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
        Err("模型设置持续变化，请稍后重试".into())
    }
    async fn exchange_copilot(&self, github_token: &str) -> Result<Session, String> {
        let response = self
            .client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("Authorization", format!("token {github_token}"))
            .header("Editor-Version", "vscode/1.104.1")
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .send()
            .await
            .map_err(|_| "GitHub Copilot 令牌服务暂时无法连接".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "GitHub Copilot 授权失败（HTTP {}），请检查订阅",
                response.status().as_u16()
            ));
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| "GitHub Copilot 令牌服务返回无效数据".to_string())?;
        let token = string(&value, "token")?;
        let base_url = value
            .pointer("/endpoints/api")
            .and_then(Value::as_str)
            .map(str::to_string);
        if base_url
            .as_deref()
            .is_some_and(|url| !trusted_copilot_base(url))
        {
            return Err("GitHub Copilot 返回了不受信任的推理地址".to_string());
        }
        Ok(Session {
            access_token: token,
            refresh_token: Some(github_token.to_string()),
            expires_at: number(&value, "expires_at", now() + 1800),
            account_id: None,
            invalid: false,
            base_url,
            account_scope: format!(
                "account-{}",
                crate::provider_auth_catalog::key(github_token)
            ),
        })
    }
    async fn handle(self: &Arc<Self>, action: &str, body: &Value) -> Result<Value, String> {
        if matches!(
            action,
            "usage"
                | "usage-history"
                | "usage-login"
                | "reset-prepare"
                | "reset-consume"
                | "reset-status"
        ) {
            if string(body, "provider")? != "openai-codex" {
                return Err("此功能仅适用于 Codex 账号".into());
            }
            // Keep account changes and the whole operation serialized, including reset settlement.
            let _guard = self.refresh.lock().await;
            if action != "reset-status" {
                self.resolve_token_locked("openai-codex", false, false)
                    .await?;
            }
            let session = self
                .session("openai-codex")
                .await?
                .filter(|s| !s.invalid)
                .ok_or("请先登录 Codex 模型账号")?;
            if body
                .get("accountScope")
                .and_then(Value::as_str)
                .is_some_and(|scope| scope != session.account_scope)
            {
                return Err("账号已切换，请重新读取当前账号用量".into());
            }
            let account_id = session
                .account_id
                .as_deref()
                .ok_or("Codex 账号缺少可核验身份，请重新登录")?;
            let expected_id = account_id.to_string();
            let expected_scope = session.account_scope.clone();
            let weak = Arc::downgrade(self);
            let refresh: crate::codex_account::TokenRefresher = Arc::new(move || {
                let weak = weak.clone();
                let expected_id = expected_id.clone();
                let expected_scope = expected_scope.clone();
                Box::pin(async move {
                    let auth = weak.upgrade().ok_or("模型账号服务已退出")?;
                    let before = auth
                        .session("openai-codex")
                        .await?
                        .ok_or("模型账号尚未登录")?;
                    if before.account_id.as_deref() != Some(&expected_id)
                        || before.account_scope != expected_scope
                        || before.invalid
                    {
                        return Err("模型账号已变化，拒绝向旧用量连接提供凭据".into());
                    }
                    auth.resolve_token_locked("openai-codex", true, false)
                        .await?;
                    let refreshed = auth
                        .session("openai-codex")
                        .await?
                        .ok_or("模型账号尚未登录")?;
                    if refreshed.account_id.as_deref() != Some(&expected_id)
                        || refreshed.account_scope != expected_scope
                        || refreshed.invalid
                    {
                        return Err("刷新后的模型账号身份不一致".into());
                    }
                    Ok(crate::codex_account::AccountTokens {
                        access_token: refreshed.access_token,
                        account_id: expected_id,
                    })
                })
            });
            let result = self
                .account_usage
                .handle_authenticated(
                    &session.account_scope,
                    account_id,
                    &session.access_token,
                    action,
                    body,
                    Some(refresh),
                )
                .await;
            let current = self.session("openai-codex").await?;
            if current.as_ref().is_none_or(|current| {
                current.invalid
                    || current.account_id.as_deref() != Some(account_id)
                    || current.account_scope != session.account_scope
            }) {
                return Err("模型登录身份已变化，已停止显示旧账号用量；请重新读取当前账号".into());
            }
            return result;
        }
        let cli = self.cli.read().clone();
        if let Some(cli) = &cli {
            if body.get("provider").and_then(Value::as_str) == Some("claude-code") {
                return match action {
                    "start" => cli.start().await,
                    "connect" => Ok(cli.status().await),
                    _ => Err("请在官方 Claude Code 客户端管理此账号".to_string()),
                };
            }
            if let Some(attempt) = body
                .get("attempt")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("claude-cli-"))
            {
                return match action {
                    "poll" => cli.poll(attempt).await,
                    "cancel" => cli.cancel(attempt).await,
                    _ => Err("未知官方客户端登录操作".to_string()),
                };
            }
        }
        match action {
            "providers" => {
                let _guard = self.refresh.lock().await;
                let mut values = Vec::new();
                for p in PROVIDERS {
                    let (session, saved) = match async {
                        Ok::<_, String>((
                            self.session(p.id).await?,
                            self.account_sessions(p.id).await?,
                        ))
                    }
                    .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            values.push(json!({"id":p.id,"name":p.name,"signedIn":false,"settingsNs":"llm-pi-ai","settingsPath":["providers",p.id],"accounts":[],"accountCount":0,"error":error}));
                            continue;
                        }
                    };
                    let scope = session
                        .as_ref()
                        .filter(|s| !s.invalid)
                        .map(|s| s.account_scope.as_str())
                        .unwrap_or("signed-out");
                    let catalog = self.catalogs.get(p.id, scope);
                    let accounts = saved.iter().map(|item| {
                        let claims = jwt(&item.access_token);
                        let label = claims.as_ref().and_then(|v| v.get("email").or_else(|| v.get("https://api.openai.com/profile").and_then(|v| v.get("email")))).and_then(Value::as_str);
                        json!({"accountScope":item.account_scope,"accountId":item.account_id,"label":label.or(item.account_id.as_deref()),"expiresAt":item.expires_at,"needsLogin":item.invalid || item.expires_at <= now() + 120 && item.refresh_token.is_none(),"active":session.as_ref().is_some_and(|current| current.account_scope == item.account_scope)})
                    }).collect::<Vec<_>>();
                    let account_count = accounts.len();
                    values.push(json!({"id":p.id,"name":p.name,"signedIn":session.as_ref().is_some_and(|s| !s.invalid),
                        "expiresAt":session.as_ref().map(|s|s.expires_at),"settingsNs":"llm-pi-ai","settingsPath":["providers",p.id],
                        "accountScope":scope,"accounts":accounts,"accountCount":account_count,"catalog":catalog.status_value()}));
                }
                if let Some(cli) = cli {
                    values.push(cli.status().await);
                }
                Ok(json!({"providers":values}))
            }
            "models" => self.model_view(&string(body, "provider")?).await,
            "switch" => {
                let id = string(body, "provider")?;
                let scope = string(body, "accountScope")?;
                let p = provider(&id)?;
                let _guard = self.refresh.lock().await;
                let selected = self
                    .account_sessions(&id)
                    .await?
                    .into_iter()
                    .find(|item| item.account_scope == scope && !item.invalid)
                    .ok_or("未找到可切换的登录账号")?;
                if id == "openai-codex" {
                    self.account_usage.disconnect().await;
                }
                self.activate(p, &selected).await?;
                Ok(
                    json!({"status":"switched","provider":id,"accountScope":selected.account_scope,"accountId":selected.account_id,"expiresAt":selected.expires_at}),
                )
            }
            "refresh" => self.refresh_catalog(&string(body, "provider")?).await,
            "start" => self.start(&string(body, "provider")?).await,
            "connect" => {
                let id = string(body, "provider")?;
                {
                    let _guard = self.refresh.lock().await;
                    self.resolve_token_locked(&id, false, true).await?;
                    let session = self
                        .session(&id)
                        .await?
                        .ok_or_else(|| "账号尚未登录".to_string())?;
                    self.install_profile(provider(&id)?, &session).await?;
                }
                self.refresh_catalog(&id).await
            }
            "poll" => self.poll(&string(body, "attempt")?).await,
            "cancel" => {
                let _guard = self.refresh.lock().await;
                let removed = self
                    .pending
                    .lock()
                    .remove(&string(body, "attempt")?)
                    .is_some();
                Ok(json!({"status":if removed { "cancelled" } else { "complete" }}))
            }
            "logout" => {
                let id = string(body, "provider")?;
                let p = provider(&id)?;
                let _guard = self.refresh.lock().await;
                let current = self.session(&id).await?;
                let scope = body
                    .get("accountScope")
                    .and_then(Value::as_str)
                    .or_else(|| current.as_ref().map(|item| item.account_scope.as_str()));
                let mut accounts = self.account_sessions(&id).await?;
                if scope
                    .is_some_and(|scope| !accounts.iter().any(|item| item.account_scope == scope))
                {
                    return Err("未找到要移除的登录账号".into());
                }
                accounts.retain(|item| Some(item.account_scope.as_str()) != scope);
                let remove_active = current
                    .as_ref()
                    .is_none_or(|item| Some(item.account_scope.as_str()) == scope);
                if remove_active && id == "openai-codex" {
                    self.account_usage.disconnect().await;
                }
                self.pending.lock().retain(|_, (owner, _)| owner.id != id);
                if remove_active {
                    self.credentials.unset(&reference(&id)).await?;
                }
                self.write_accounts(&id, &accounts).await?;
                if remove_active {
                    self.catalogs.unbind(&id);
                    if let Some(next) = accounts.iter().find(|item| !item.invalid) {
                        self.activate(p, next).await?;
                    }
                }
                Ok(json!({"status":"signedOut","removedAccountScope":scope}))
            }
            _ => Err("未知账号操作".to_string()),
        }
    }
    pub(crate) fn register(self: &Arc<Self>, server: &Arc<WebServer>) -> RouteDisposer {
        let auth = self.clone();
        server.register(WebRoute {
            kind: WebRouteKind::Prefix,
            path: "/provider-auth".to_string(),
            handler: Arc::new(move |request| {
                let auth = auth.clone();
                Box::pin(async move {
                    let forbidden = !super::trusted_web_request(&request, false);
                    let method = request.method().clone();
                    let action = request
                        .uri()
                        .path()
                        .trim_start_matches("/provider-auth/")
                        .to_string();
                    let (status, value) = if forbidden {
                        (403, json!({"error":"forbidden"}))
                    } else if method != http::Method::POST {
                        (405, json!({"error":"method not allowed"}))
                    } else {
                        match axum::body::to_bytes(axum::body::Body::new(request.into_body()), 8192)
                            .await
                        {
                            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                                Ok(body) => match auth.handle(&action, &body).await {
                                    Ok(value) => (200, value),
                                    Err(error) => (400, json!({"error":error})),
                                },
                                Err(_) => (400, json!({"error":"invalid JSON"})),
                            },
                            Err(_) => (413, json!({"error":"request too large"})),
                        }
                    };
                    Ok(http::Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .header("cache-control", "no-store")
                        .body(axum::body::Body::from(value.to_string()))
                        .expect("static response"))
                })
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_refresh_rejects_only_confirmed_revoked_grants_and_preserves_service_failures() {
        assert!(codex_refresh_requires_login(
            400,
            &json!({"error":"invalid_grant"})
        ));
        assert!(codex_refresh_requires_login(
            401,
            &json!({"error":{"code":"refresh_token_reused"}})
        ));
        for status in [400, 401, 403, 429, 500, 503] {
            assert!(!codex_refresh_requires_login(
                status,
                &json!({"error":"service_unavailable"})
            ));
        }
        assert!(!codex_refresh_requires_login(
            403,
            &json!({"error":"invalid_grant"})
        ));
        assert!(!codex_refresh_requires_login(
            400,
            &json!({"error":"invalid_request"})
        ));
    }
    #[test]
    fn credentials_cannot_follow_a_profile_to_an_arbitrary_endpoint() {
        assert!(valid_profile(
            "nous",
            "https://inference-api.nousresearch.com/v1",
            "openai-completions"
        ));
        assert!(!valid_profile(
            "nous",
            "https://example.com/v1",
            "openai-completions"
        ));
        assert!(!valid_profile(
            "openai-codex",
            "https://chatgpt.com/backend-api/codex",
            "openai-completions"
        ));
    }
    #[test]
    fn refresh_preserves_unrotated_token_and_rejects_missing_access_token() {
        let original = Session::from_tokens(
            &json!({"access_token":"original","refresh_token":"renew","expires_in":3600}),
            None,
        )
        .unwrap();
        let renewed = Session::from_tokens(
            &json!({"access_token":"next","expires_in":1800}),
            Some(&original),
        )
        .unwrap();
        assert_eq!(renewed.refresh_token.as_deref(), Some("renew"));
        assert!(Session::from_tokens(&json!({"error":"invalid_grant"}), Some(&original)).is_err());
    }
    #[test]
    fn pkce_challenges_and_expiry_follow_provider_formats() {
        let (verifier, challenge) = pkce();
        assert!(verifier.len() >= 43);
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
        assert_ne!(pkce().0, verifier);
        let epoch = (now() + 300) * 1000;
        assert_eq!(expiry(&json!({"expired_in":epoch})), epoch / 1000);
        assert!((now() + 10..=now() + 12).contains(&expiry(&json!({"expired_in":11}))));
    }
    #[test]
    fn copilot_rejects_untrusted_exchange_endpoints() {
        assert!(trusted_copilot_base(
            "https://api.business.githubcopilot.com"
        ));
        for url in [
            "https://githubcopilot.com.evil.test",
            "http://api.githubcopilot.com",
            "https://api.githubcopilot.com@evil.test",
            "https://api.githubcopilot.com:1234",
        ] {
            assert!(!trusted_copilot_base(url));
        }
    }
}
