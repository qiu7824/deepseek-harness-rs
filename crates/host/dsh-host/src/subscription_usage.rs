//! Read-only account allowances, kept separate from session token statistics.
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(60);
const RESPONSE_LIMIT: usize = 512 * 1024;
const NOUS_ACCOUNT: &str = "https://portal.nousresearch.com/api/oauth/account";
const COPILOT_ACCOUNT: &str = "https://api.github.com/copilot_internal/user";
const GROK_BILLING: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const GROK_IDENTITY: &str = "https://auth.x.ai/oauth2/userinfo";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn nonblank(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn percent(value: &Value) -> Option<f64> {
    number(value).filter(|number| *number <= 100.0)
}

fn timestamp(value: &Value) -> Option<u64> {
    if let Some(seconds) = value
        .as_u64()
        .filter(|seconds| *seconds > 0 && *seconds <= 253_402_300_799)
    {
        return Some(seconds);
    }
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .and_then(|date| u64::try_from(date.timestamp()).ok())
        .filter(|seconds| *seconds > 0)
}

fn window(
    id: &str,
    label: &str,
    used_percent: Option<f64>,
    remaining_percent: Option<f64>,
    resets_at: Option<u64>,
    duration: Option<f64>,
) -> Value {
    json!({"id":id,"label":label,"usedPercent":used_percent,"remainingPercent":remaining_percent,"resetsAt":resets_at,"windowDurationMins":duration,"used":null,"remaining":null,"limit":null,"unit":"percent"})
}

fn report(
    provider: &str,
    scope: &str,
    status: &str,
    plan: Option<String>,
    windows: Vec<Value>,
    updated_at: Option<u64>,
    message: Option<&str>,
) -> Value {
    let mut report = json!({"provider":provider,"accountScope":scope,"status":status,"plan":plan,"updatedAt":updated_at,"windows":windows});
    if let Some(message) = message {
        report["message"] = json!(message);
    }
    report
}

fn unavailable(provider: &str, scope: &str, status: &str, message: &str) -> Value {
    report(
        provider,
        scope,
        status,
        None,
        Vec::new(),
        None,
        Some(message),
    )
}

fn readable(windows: &[Value]) -> bool {
    windows.iter().any(|row| {
        [
            "usedPercent",
            "remainingPercent",
            "used",
            "remaining",
            "limit",
        ]
        .iter()
        .any(|field| row.get(*field).is_some_and(|value| value.is_number()))
    })
}

/// Normalize the official app-server response without inventing missing quotas.
pub(crate) fn report_from_codex(provider: &str, account_scope: &str, raw: &Value) -> Value {
    if let Some(error) = raw.get("error").and_then(Value::as_str)
        && (error.contains("HTTP 401") || error.contains("HTTP 403"))
    {
        let mut value = codex_failure_report(account_scope, error);
        value["provider"] = json!(provider);
        return value;
    }
    let mut windows = Vec::new();
    let mut plan = nonblank(&raw["planType"]);
    let buckets: Vec<(&str, &Value)> = match raw
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|buckets| !buckets.is_empty())
    {
        Some(buckets) => buckets
            .iter()
            .take(16)
            .map(|(id, value)| (id.as_str(), value))
            .collect(),
        None => raw
            .get("rateLimits")
            .filter(|value| value.is_object())
            .map(|value| vec![(value["limitId"].as_str().unwrap_or("codex"), value)])
            .unwrap_or_default(),
    };
    for (id, bucket) in buckets {
        if plan.is_none() {
            plan = nonblank(&bucket["planType"]);
        }
        let bucket_label = nonblank(&bucket["limitName"]).unwrap_or_else(|| {
            if id == "codex" {
                "Codex".into()
            } else {
                id.chars().take(128).collect()
            }
        });
        for (key, fallback) in [("primary", "主额度窗口"), ("secondary", "附加额度窗口")]
        {
            let Some(value) = bucket.get(key).filter(|value| value.is_object()) else {
                continue;
            };
            let duration = number(&value["windowDurationMins"]).filter(|minutes| *minutes > 0.0);
            let label = match duration {
                Some(minutes) if minutes == 300.0 => format!("{bucket_label} · 5 小时"),
                Some(minutes) if minutes == 10080.0 => format!("{bucket_label} · 7 天"),
                Some(minutes) if minutes < 60.0 => format!("{bucket_label} · {minutes} 分钟"),
                Some(minutes) => format!("{bucket_label} · {} 小时", minutes / 60.0),
                None => format!("{bucket_label} · {fallback}"),
            };
            let used = percent(&value["usedPercent"]);
            windows.push(window(
                &format!("{id}:{key}"),
                &label,
                used,
                used.map(|used| 100.0 - used),
                timestamp(&value["resetsAt"]),
                duration,
            ));
        }
    }
    let supplied_status = raw["status"]
        .as_str()
        .filter(|status| matches!(*status, "fresh" | "stale" | "unavailable" | "needsLogin"));
    let status = supplied_status.unwrap_or(if readable(&windows) {
        "fresh"
    } else {
        "unavailable"
    });
    let updated_at = timestamp(&raw["updatedAt"]).or_else(|| (status == "fresh").then(now));
    let message = if status == "stale" {
        Some("显示上次获取的额度，刷新暂时失败")
    } else if !readable(&windows) {
        Some("账户服务未提供可读取的额度数值")
    } else {
        None
    };
    report(
        provider,
        account_scope,
        status,
        plan,
        windows,
        updated_at,
        message,
    )
}

/// Safe failure DTO for the account panel; the existing Codex tools keep their own protocol.
pub(crate) fn codex_failure_report(account_scope: &str, error: &str) -> Value {
    let forbidden = error.contains("HTTP 403");
    let requires_login = !forbidden
        && (error.contains("HTTP 401")
            || [
                "尚未登录",
                "请先登录",
                "授权已失效",
                "授权已过期",
                "凭据已失效",
                "缺少可核验身份",
                "缺少可用的登录身份",
            ]
            .iter()
            .any(|needle| error.contains(needle)));
    let mut value = unavailable(
        "openai-codex",
        account_scope,
        if requires_login {
            "needsLogin"
        } else {
            "unavailable"
        },
        if forbidden {
            "Codex 额度服务拒绝访问，请检查账号权限或服务限制"
        } else if requires_login {
            "Codex 账号认证已失效，请重新登录后读取额度"
        } else {
            "Codex 额度查询失败，请检查官方客户端和账号连接"
        },
    );
    value["clearSnapshot"] = json!(true);
    value
}

#[derive(Clone)]
struct Snapshot {
    value: Value,
    fetched: Instant,
}

#[derive(Debug)]
struct ReadError {
    status: Option<u16>,
    message: &'static str,
}

impl ReadError {
    fn transport() -> Self {
        Self {
            status: None,
            message: "额度服务连接失败，请稍后刷新",
        }
    }
    fn malformed() -> Self {
        Self {
            status: None,
            message: "额度服务返回了无法识别的数据",
        }
    }
    fn http(status: u16) -> Self {
        let message = match status {
            401 => "额度服务认证已失效，请重新登录此账号",
            403 => "额度服务拒绝访问，此账号暂时无法读取额度",
            404 => "此账号的额度接口暂不可用",
            429 => "额度查询过于频繁，请稍后刷新",
            426 => "额度服务要求更新客户端，暂时无法读取",
            _ => "额度服务暂时不可用，请稍后刷新",
        };
        Self {
            status: Some(status),
            message,
        }
    }
}

pub(crate) struct SubscriptionUsage {
    cache: Mutex<HashMap<String, Snapshot>>,
}

impl SubscriptionUsage {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Credentials stay in memory and are sent only to the fixed account service.
    pub(crate) async fn read(
        &self,
        provider: &str,
        account_scope: &str,
        token: &str,
        github_refresh_token: Option<&str>,
        force: bool,
    ) -> Value {
        if !matches!(provider, "devin" | "nous" | "copilot" | "xai-oauth") {
            let message = match provider {
                "claude-code" | "anthropic" => {
                    "Claude Code 由官方客户端托管登录；独立结构化额度查询暂不支持，可在官方客户端运行 /usage"
                }
                "qwen-oauth" => "此 Qwen OAuth 登录尚无已验证的账号额度接口",
                "minimax-oauth" | "minimax-cn-oauth" => {
                    "此 MiniMax OAuth 登录的额度接口兼容性尚未验证"
                }
                "openai-codex" => "请通过 Codex 官方账户服务读取订阅额度",
                _ => "此订阅账号尚不支持独立额度查询",
            };
            return unavailable(provider, account_scope, "unsupported", message);
        }
        let credential = if provider == "copilot" {
            github_refresh_token.filter(|value| !value.trim().is_empty())
        } else {
            Some(token).filter(|value| !value.trim().is_empty())
        };
        let Some(credential) = credential else {
            return unavailable(
                provider,
                account_scope,
                "needsLogin",
                if provider == "copilot" {
                    "缺少已登录的 GitHub 账号凭据，请重新登录 Copilot"
                } else {
                    "请先登录此订阅账号"
                },
            );
        };
        let mut digest = Sha256::new();
        digest.update(token.as_bytes());
        digest.update([0]);
        digest.update(credential.as_bytes());
        let fingerprint = format!("{:x}", digest.finalize());
        let cache_prefix = format!("{provider}\0{account_scope}\0");
        let cache_key = format!("{cache_prefix}{fingerprint}");
        let previous = {
            let mut cache = self.cache.lock().await;
            cache.retain(|key, _| !key.starts_with(&cache_prefix) || key == &cache_key);
            cache.get(&cache_key).cloned()
        };
        if !force
            && let Some(snapshot) = &previous
            && snapshot.fetched.elapsed() < TTL
        {
            return snapshot.value.clone();
        }
        let fetched =
            tokio::time::timeout(Duration::from_secs(30), self.fetch(provider, credential))
                .await
                .unwrap_or_else(|_| Err(ReadError::transport()));
        match fetched {
            Ok(raw) => {
                let (plan, windows) = match provider {
                    "devin" => parse_devin(&raw),
                    "nous" => parse_nous(&raw),
                    "copilot" => parse_copilot(&raw),
                    "xai-oauth" => parse_grok(&raw),
                    _ => unreachable!(),
                };
                let has_numbers = readable(&windows);
                let value = report(
                    provider,
                    account_scope,
                    if has_numbers { "fresh" } else { "unavailable" },
                    plan,
                    windows,
                    Some(now()),
                    if has_numbers {
                        None
                    } else {
                        Some("服务端未提供可读取的额度数值")
                    },
                );
                let mut cache = self.cache.lock().await;
                if cache.len() >= 64 && !cache.contains_key(&cache_key) {
                    if let Some(oldest) = cache
                        .iter()
                        .max_by_key(|(_, snapshot)| snapshot.fetched.elapsed())
                        .map(|(key, _)| key.clone())
                    {
                        cache.remove(&oldest);
                    }
                }
                cache.insert(
                    cache_key,
                    Snapshot {
                        value: value.clone(),
                        fetched: Instant::now(),
                    },
                );
                value
            }
            Err(error) => {
                self.failure_report(provider, account_scope, &cache_key, previous, error)
                    .await
            }
        }
    }

    async fn failure_report(
        &self,
        provider: &str,
        account_scope: &str,
        cache_key: &str,
        previous: Option<Snapshot>,
        error: ReadError,
    ) -> Value {
        if matches!(error.status, Some(401 | 403)) {
            self.cache.lock().await.remove(cache_key);
            let mut value = unavailable(
                provider,
                account_scope,
                if error.status == Some(401) {
                    "needsLogin"
                } else {
                    "unavailable"
                },
                error.message,
            );
            value["clearSnapshot"] = json!(true);
            return value;
        }
        if let Some(snapshot) = previous.filter(|snapshot| snapshot.value["status"] == "fresh") {
            let mut value = snapshot.value;
            value["status"] = json!("stale");
            value["message"] = json!("显示上次获取的额度，刷新暂时失败");
            return value;
        }
        unavailable(provider, account_scope, "unavailable", error.message)
    }

    async fn fetch(&self, provider: &str, token: &str) -> Result<Value, ReadError> {
        if provider == "devin" {
            return dsh_llm_deepseek::devin::read_usage(dsh_llm_deepseek::devin::BASE_URL, token)
                .await
                .map_err(|error| {
                    if error.contains("401") {
                        ReadError::http(401)
                    } else if error.contains("403") {
                        ReadError::http(403)
                    } else {
                        ReadError::transport()
                    }
                });
        }
        let client = dsh_http_proxy::builder()
            .map_err(|_| ReadError::transport())?
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent("DeepSeek-Harness-rs")
            .build()
            .map_err(|_| ReadError::transport())?;
        match provider {
            "nous" => {
                read_json(
                    client
                        .get(NOUS_ACCOUNT)
                        .bearer_auth(token)
                        .header("accept", "application/json"),
                )
                .await
            }
            "copilot" => {
                read_json(
                    client
                        .get(COPILOT_ACCOUNT)
                        .bearer_auth(token)
                        .header("accept", "application/json")
                        .header("x-github-api-version", "2022-11-28"),
                )
                .await
            }
            "xai-oauth" => {
                let subject = match jwt_subject(token) {
                    Some(subject) => subject,
                    None => {
                        let identity = read_json(
                            client
                                .get(GROK_IDENTITY)
                                .bearer_auth(token)
                                .header("accept", "application/json"),
                        )
                        .await?;
                        nonblank(&identity["sub"]).ok_or_else(ReadError::malformed)?
                    }
                };
                read_json(
                    client
                        .get(GROK_BILLING)
                        .bearer_auth(token)
                        .header("accept", "application/json")
                        .header("x-xai-token-auth", "xai-grok-cli")
                        .header("x-userid", subject)
                        .header("x-grok-client-version", env!("CARGO_PKG_VERSION"))
                        .header("x-grok-client-identifier", "DeepSeek-Harness-rs")
                        .header("x-grok-client-mode", "headless"),
                )
                .await
            }
            _ => Err(ReadError::malformed()),
        }
    }
}

async fn read_json(request: reqwest::RequestBuilder) -> Result<Value, ReadError> {
    let mut response = request.send().await.map_err(|_| ReadError::transport())?;
    if !response.status().is_success() {
        return Err(ReadError::http(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(ReadError::malformed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ReadError::transport())? {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(ReadError::malformed());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ReadError::malformed())?;
    if !value.is_object() {
        return Err(ReadError::malformed());
    }
    Ok(value)
}

fn jwt_subject(token: &str) -> Option<String> {
    if token.len() > 64 * 1024 {
        return None;
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    nonblank(&claims["sub"])
}

fn parse_devin(raw: &Value) -> (Option<String>, Vec<Value>) {
    let mut windows = Vec::new();
    for value in raw["windows"].as_array().into_iter().flatten().take(64) {
        let id = nonblank(&value["id"]).unwrap_or_else(|| format!("devin:{}", windows.len()));
        let label = nonblank(&value["label"]).unwrap_or_else(|| "Devin 额度".into());
        let remaining = percent(&value["remainingPercent"]);
        let used =
            percent(&value["usedPercent"]).or_else(|| remaining.map(|remaining| 100.0 - remaining));
        windows.push(window(
            &id,
            &label,
            used,
            remaining.or_else(|| used.map(|used| 100.0 - used)),
            timestamp(&value["resetsAt"]),
            number(&value["windowDurationMins"]).filter(|minutes| *minutes > 0.0),
        ));
    }
    (nonblank(&raw["plan"]), windows)
}

// Contract: NousResearch/hermes-agent, hermes_cli/nous_account.py.
fn parse_nous(raw: &Value) -> (Option<String>, Vec<Value>) {
    let subscription = &raw["subscription"];
    let mut windows = Vec::new();
    if subscription.is_object() {
        let mut row = window("nous:subscription", "订阅额度", None, None, None, None);
        row["unit"] = json!("credits");
        row["periodEndsAt"] = json!(timestamp(&subscription["current_period_end"]));
        row["remaining"] = json!(
            number(&subscription["credits_remaining"])
                .or_else(|| number(&raw["paid_service_access"]["subscription_credits_remaining"]))
        );
        row["limit"] = json!(number(&subscription["monthly_credits"]));
        // Remaining credits can include rollover: no percent inferred from the monthly grant.
        windows.push(row);
    }
    if let Some(remaining) = number(&raw["paid_service_access"]["purchased_credits_remaining"])
        .or_else(|| number(&raw["purchased_credits_remaining"]))
    {
        let mut row = window("nous:purchased", "购买额度余额", None, None, None, None);
        row["unit"] = json!("credits");
        row["remaining"] = json!(remaining);
        windows.push(row);
    }
    (nonblank(&subscription["plan"]), windows)
}

// Contract: microsoft/vscode-copilot-chat, platform/chat/common/chatQuotaService.ts.
fn parse_copilot(raw: &Value) -> (Option<String>, Vec<Value>) {
    let mut windows = Vec::new();
    let reset =
        timestamp(&raw["quota_reset_date_utc"]).or_else(|| timestamp(&raw["quota_reset_date"]));
    for (key, label) in [
        ("premium_models", "高级模型额度"),
        ("premium_interactions", "高级用量额度"),
        ("chat", "聊天额度"),
        ("completions", "补全额度"),
    ] {
        let Some(value) = raw["quota_snapshots"]
            .get(key)
            .filter(|value| value.is_object())
        else {
            continue;
        };
        if value["unlimited"] == true {
            continue;
        }
        let remaining = percent(&value["percent_remaining"]);
        windows.push(window(
            &format!("copilot:{key}"),
            label,
            remaining.map(|remaining| 100.0 - remaining),
            remaining,
            timestamp(&value["reset_date"]).or(reset),
            None,
        ));
    }
    // Different billing generations use different entitlement units. Only the declared percentage is shared.
    (nonblank(&raw["copilot_plan"]), windows)
}

fn cents(value: &Value) -> Option<f64> {
    let object = value.as_object()?;
    if object.is_empty() {
        return Some(0.0);
    } // Official proto3 JSON omits a zero `val`.
    number(&value["val"]).map(|cents| cents / 100.0)
}

// Contract: xai-org/grok-build, xai-grok-shell/src/extensions/billing.rs.
fn parse_grok(raw: &Value) -> (Option<String>, Vec<Value>) {
    let config = &raw["config"];
    if !config.is_object() {
        return (None, Vec::new());
    }
    let period = &config["currentPeriod"];
    let start = timestamp(&period["start"]).or_else(|| timestamp(&config["billingPeriodStart"]));
    let end = timestamp(&period["end"]).or_else(|| timestamp(&config["billingPeriodEnd"]));
    let duration = start
        .zip(end)
        .filter(|(start, end)| end > start)
        .map(|(start, end)| (end - start) as f64 / 60.0);
    let label = match period["type"].as_str() {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => "每周套餐额度",
        Some("USAGE_PERIOD_TYPE_MONTHLY") => "每月套餐额度",
        Some("USAGE_PERIOD_TYPE_DAILY") => "每日套餐额度",
        _ => "套餐额度",
    };
    let used_percent = percent(&config["creditUsagePercent"]);
    let mut row = window(
        "grok:included",
        label,
        used_percent,
        used_percent.map(|used| 100.0 - used),
        end,
        duration,
    );
    if !config
        .as_object()
        .is_some_and(|object| object.contains_key("creditUsagePercent"))
    {
        let used = cents(&config["used"]);
        let limit = cents(&config["monthlyLimit"]);
        row["unit"] = json!("usd");
        row["used"] = json!(used);
        row["limit"] = json!(limit);
        if let (Some(used), Some(limit)) = (used, limit) {
            row["remaining"] = json!((limit - used).max(0.0));
            if limit > 0.0 && used <= limit {
                row["usedPercent"] = json!(100.0 * used / limit);
                row["remainingPercent"] = json!(100.0 * (limit - used) / limit);
            }
        }
    }
    let mut windows = vec![row];
    if let Some(remaining) = cents(&config["prepaidBalance"]) {
        let mut row = window("grok:prepaid", "购买额度余额", None, None, None, None);
        row["unit"] = json!("usd");
        row["remaining"] = json!(remaining);
        windows.push(row);
    }
    (
        nonblank(&raw["subscriptionTier"]).or_else(|| nonblank(&config["subscriptionTier"])),
        windows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_prefers_multiple_buckets_and_preserves_missing_fields() {
        let value = report_from_codex(
            "openai-codex",
            "main",
            &json!({"planType":" ","rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"codex":{"planType":"pro","primary":{"usedPercent":1,"windowDurationMins":10080,"resetsAt":1800000000}},"spark":{"secondary":{"windowDurationMins":60}}}}),
        );
        assert_eq!(value["plan"], "pro");
        assert_eq!(value["windows"].as_array().unwrap().len(), 2);
        assert_eq!(value["windows"][0]["remainingPercent"], 99.0);
        assert!(
            value["windows"][0]["label"]
                .as_str()
                .unwrap()
                .contains("7 天")
        );
        assert!(value["windows"][1]["usedPercent"].is_null());
        assert!(value["windows"][1]["resetsAt"].is_null());
    }

    #[test]
    fn percentages_are_not_strings_clamped_or_scaled() {
        for used in [json!(-1), json!(101), json!("25"), Value::Null] {
            let value = report_from_codex(
                "openai-codex",
                "main",
                &json!({"rateLimits":{"primary":{"usedPercent":used}}}),
            );
            assert!(value["windows"][0]["usedPercent"].is_null());
            assert!(value["windows"][0]["remainingPercent"].is_null());
        }
        let value = report_from_codex(
            "openai-codex",
            "main",
            &json!({"rateLimits":{"primary":{"usedPercent":0.5}}}),
        );
        assert_eq!(value["windows"][0]["usedPercent"], 0.5);
    }

    #[test]
    fn codex_auth_failure_inside_old_stale_response_clears_the_panel() {
        for status in [401, 403] {
            let value = report_from_codex(
                "openai-codex",
                "main",
                &json!({"status":"stale","updatedAt":1800000000,"error":format!("账户服务失败（HTTP {status}）private-token"),"rateLimits":{"planType":"pro","primary":{"usedPercent":25}}}),
            );
            assert_eq!(
                value["status"],
                if status == 401 {
                    "needsLogin"
                } else {
                    "unavailable"
                }
            );
            assert_eq!(value["clearSnapshot"], true);
            assert!(value["plan"].is_null());
            assert!(value["windows"].as_array().unwrap().is_empty());
            assert!(!value.to_string().contains("private-token"));
        }
        let value = report_from_codex(
            "openai-codex",
            "main",
            &json!({"status":"stale","updatedAt":1800000000,"error":"账户服务暂时不可用（HTTP 503）","rateLimits":{"planType":"pro","primary":{"usedPercent":25}}}),
        );
        assert_eq!(value["status"], "stale");
        assert_eq!(value["windows"][0]["remainingPercent"], 75.0);
        assert_eq!(value["updatedAt"], 1800000000_u64);
        assert!(value["clearSnapshot"].is_null());
    }

    #[test]
    fn codex_direct_failures_always_clear_and_never_copy_error_payloads() {
        for error in [
            "账号授权已失效，请重新登录 private-token",
            "账号尚未登录 private-token",
            "HTTP 401 private-token",
        ] {
            let value = codex_failure_report("main", error);
            assert_eq!(value["status"], "needsLogin");
            assert_eq!(value["clearSnapshot"], true);
            assert!(!value.to_string().contains("private-token"));
        }
        for error in [
            "HTTP 403 private-token",
            "HTTP 503 private-token",
            "无法启动 Codex 账户服务 private-token",
        ] {
            let value = codex_failure_report("main", error);
            assert_eq!(value["status"], "unavailable");
            assert_eq!(value["clearSnapshot"], true);
            assert!(value["windows"].as_array().unwrap().is_empty());
            assert!(!value.to_string().contains("private-token"));
        }
    }

    #[test]
    fn codex_limits_multi_bucket_output_to_thirty_two_windows() {
        let buckets: serde_json::Map<String, Value> = (0..20)
            .map(|index| {
                (
                    format!("bucket-{index:02}"),
                    json!({"primary":{"usedPercent":25},"secondary":{"usedPercent":50}}),
                )
            })
            .collect();
        let value = report_from_codex(
            "openai-codex",
            "main",
            &json!({"rateLimitsByLimitId":buckets}),
        );
        assert_eq!(value["windows"].as_array().unwrap().len(), 32);
    }

    #[tokio::test]
    async fn dispatch_uses_the_harness_provider_identifiers_without_network() {
        let service = SubscriptionUsage::new();
        let copilot = service.read("copilot", "main", "", None, false).await;
        assert_eq!(copilot["status"], "needsLogin");
        assert!(copilot["message"].as_str().unwrap().contains("GitHub"));
        for provider in ["qwen-oauth", "minimax-oauth", "minimax-cn-oauth"] {
            let value = service.read(provider, "main", "", None, false).await;
            assert_eq!(value["status"], "unsupported");
            assert!(
                value["message"]
                    .as_str()
                    .unwrap()
                    .contains(if provider == "qwen-oauth" {
                        "Qwen"
                    } else {
                        "MiniMax"
                    })
            );
        }
    }

    #[tokio::test]
    async fn auth_failures_clear_the_cache_and_instruct_the_ui_to_drop_old_numbers() {
        let service = SubscriptionUsage::new();
        let old = Snapshot {
            value: report(
                "copilot",
                "main",
                "fresh",
                Some("pro".into()),
                vec![window("quota", "额度", Some(20.0), Some(80.0), None, None)],
                Some(1800000000),
                None,
            ),
            fetched: Instant::now(),
        };
        for status in [401, 403] {
            service
                .cache
                .lock()
                .await
                .insert("credential".into(), old.clone());
            let value = service
                .failure_report(
                    "copilot",
                    "main",
                    "credential",
                    Some(old.clone()),
                    ReadError::http(status),
                )
                .await;
            assert_eq!(
                value["status"],
                if status == 401 {
                    "needsLogin"
                } else {
                    "unavailable"
                }
            );
            assert_eq!(value["clearSnapshot"], true);
            assert!(value["windows"].as_array().unwrap().is_empty());
            assert!(value["plan"].is_null());
            assert!(!service.cache.lock().await.contains_key("credential"));
        }
        let value = service
            .failure_report(
                "copilot",
                "main",
                "credential",
                Some(old),
                ReadError::http(503),
            )
            .await;
        assert_eq!(value["status"], "stale");
        assert_eq!(value["updatedAt"], 1800000000_u64);
        assert_eq!(value["windows"][0]["remainingPercent"], 80.0);
        assert!(value["clearSnapshot"].is_null());
    }

    #[test]
    fn copilot_keeps_new_and_old_percent_units_without_guessing_counts() {
        let (_, rows) = parse_copilot(
            &json!({"quota_snapshots":{"premium_models":{"entitlement":300,"remaining":250,"percent_remaining":25},"premium_interactions":{"percent_remaining":50},"chat":{"unlimited":true,"percent_remaining":0}}}),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["usedPercent"], 75.0);
        assert!(rows[0]["used"].is_null());
        assert!(rows[0]["limit"].is_null());
        let (_, rows) = parse_copilot(
            &json!({"limited_user_quotas":{"chat":450},"monthly_quotas":{"chat":500}}),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn grok_uses_declared_percent_and_separates_legacy_usd_cents() {
        let (_, rows) = parse_grok(
            &json!({"config":{"creditUsagePercent":42.5,"monthlyLimit":{"val":2000},"used":{"val":1234},"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-06-01T00:00:00Z","end":"2026-06-08T00:00:00Z"}}}),
        );
        assert_eq!(rows[0]["usedPercent"], 42.5);
        assert_eq!(rows[0]["unit"], "percent");
        assert!(rows[0]["limit"].is_null());
        assert_eq!(rows[0]["windowDurationMins"], 10080.0);
        let (_, rows) = parse_grok(
            &json!({"config":{"monthlyLimit":{"val":2000},"used":{"val":1234},"prepaidBalance":{}}}),
        );
        assert_eq!(rows[0]["unit"], "usd");
        assert_eq!(rows[0]["limit"], 20.0);
        assert_eq!(rows[0]["used"], 12.34);
        assert_eq!(rows[1]["remaining"], 0.0);
    }

    #[test]
    fn nous_rollover_balance_does_not_imply_a_monthly_percentage() {
        let (_, rows) = parse_nous(
            &json!({"subscription":{"plan":"Tier 2","monthly_credits":20,"credits_remaining":23.5,"rollover_credits":5.0,"current_period_end":"2026-06-01T00:00:00Z"},"purchased_credits_remaining":7.75}),
        );
        assert_eq!(rows[0]["remaining"], 23.5);
        assert_eq!(rows[0]["limit"], 20.0);
        assert_eq!(rows[0]["unit"], "credits");
        assert!(rows[0]["usedPercent"].is_null());
        assert!(rows[0]["resetsAt"].is_null());
        assert!(rows[0]["periodEndsAt"].as_u64().is_some());
        assert_eq!(rows[1]["remaining"], 7.75);
    }

    #[test]
    fn nous_uses_nested_paid_service_balances_and_preserves_real_zero() {
        let (_, rows) = parse_nous(
            &json!({"subscription":{"plan":"Tier 2","monthly_credits":20},"paid_service_access":{"subscription_credits_remaining":12.25,"purchased_credits_remaining":0},"purchased_credits_remaining":9.5}),
        );
        assert_eq!(rows[0]["remaining"], 12.25);
        assert_eq!(rows[1]["remaining"], 0.0);
        assert!(rows[0]["usedPercent"].is_null());
        let (_, rows) = parse_nous(
            &json!({"subscription":{"credits_remaining":3.25},"paid_service_access":{"purchased_credits_remaining":7.75}}),
        );
        assert_eq!(rows[0]["remaining"], 3.25);
        assert_eq!(rows[1]["remaining"], 7.75);
    }

    #[test]
    fn grok_rejects_invalid_new_percentage_without_falling_back_to_legacy_budget() {
        for used in [json!(101), json!(-1), json!("42.5"), Value::Null] {
            let (_, rows) = parse_grok(
                &json!({"config":{"creditUsagePercent":used,"monthlyLimit":{"val":2000},"used":{"val":1000}}}),
            );
            assert!(rows[0]["usedPercent"].is_null());
            assert!(rows[0]["remainingPercent"].is_null());
            assert!(rows[0]["used"].is_null());
            assert_eq!(rows[0]["unit"], "percent");
        }
        let (_, rows) = parse_grok(&json!({"monthlyLimit":{"val":2000},"used":{"val":1000}}));
        assert!(rows.is_empty());
    }
}
