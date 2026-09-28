//! Observed native-tool availability, scoped to the exact connection and credential.
//! This cache never grants permissions and never performs a billable probe.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: u64 = 512 * 1024;
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    scope: String,
    role: String,
    model: String,
    driver: Option<String>,
    operation: String,
    state: String,
    code: Option<String>,
    checked_at: u64,
    sequence: u64,
}
#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    salt: String,
    entries: BTreeMap<String, Observation>,
}
pub(super) struct Ticket {
    key: String,
    scope: String,
    role: String,
    model: String,
    driver: Option<String>,
    operation: String,
    sequence: u64,
}
pub(super) struct NativeCapabilities {
    path: PathBuf,
    state: Mutex<Document>,
    writes: tokio::sync::Mutex<()>,
    sequence: AtomicU64,
    persistent: bool,
}

impl NativeCapabilities {
    pub fn open(root: &Path) -> Self {
        let path = root.join("cache/native-capabilities-v1.json");
        let loaded = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= MAX_BYTES)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|bytes| serde_json::from_slice::<Document>(&bytes).ok());
        let persistent = loaded.as_ref().is_none_or(|doc| doc.version == 1);
        let state = loaded
            .filter(|doc| {
                doc.version == 1
                    && doc.entries.len() <= MAX_ENTRIES
                    && doc.salt.len() >= 32
                    && doc.salt.len() <= 128
            })
            .unwrap_or_else(|| Document {
                version: 1,
                salt: uuid::Uuid::new_v4().to_string(),
                entries: BTreeMap::new(),
            });
        let next = state
            .entries
            .values()
            .map(|o| o.sequence)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Self {
            path,
            state: Mutex::new(state),
            writes: tokio::sync::Mutex::new(()),
            sequence: AtomicU64::new(next),
            persistent,
        }
    }
    fn scope(&self, provider: &str, profile: &Value, credential: Option<&str>) -> String {
        // The private salt prevents exposing a reusable digest of a credential.
        let salt = self.state.lock().salt.clone();
        let network = dsh_http_proxy::policy()
            .map(|p| p.fingerprint(&salt))
            .unwrap_or_else(|_| "unavailable".into());
        let mut hash = Sha256::new();
        for value in [
            salt.as_bytes(),
            provider.as_bytes(),
            profile.to_string().as_bytes(),
            credential.unwrap_or("").as_bytes(),
            network.as_bytes(),
            std::env::consts::OS.as_bytes(),
            env!("CARGO_PKG_VERSION").as_bytes(),
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        }
        format!("{:x}", hash.finalize())
    }
    pub fn ticket(
        &self,
        provider: &str,
        profile: &Value,
        credential: Option<&str>,
        role: &str,
        model: &str,
        driver: Option<&str>,
        operation: &str,
    ) -> Ticket {
        let scope = self.scope(provider, profile, credential);
        let key = format!(
            "{:x}",
            Sha256::digest(
                json!([scope, role, model, driver, operation])
                    .to_string()
                    .as_bytes()
            )
        );
        Ticket {
            key,
            scope,
            role: role.into(),
            model: model.into(),
            driver: driver.map(str::to_owned),
            operation: operation.into(),
            sequence: self.sequence.fetch_add(1, Ordering::SeqCst),
        }
    }
    pub async fn observe(
        &self,
        ticket: &Ticket,
        state: &str,
        code: Option<&str>,
    ) -> Result<(), String> {
        self.observe_at(ticket, state, code, now()).await
    }
    pub fn preflight(&self, ticket: &Ticket) -> Result<(), String> {
        self.preflight_at(ticket, now())
    }
    fn preflight_at(&self, ticket: &Ticket, at: u64) -> Result<(), String> {
        let state = self.state.lock();
        let Some(entry) = state.entries.get(&ticket.key) else {
            return Ok(());
        };
        if at < entry.checked_at || at - entry.checked_at >= 60 {
            return Ok(());
        }
        let code = match entry.state.as_str() {
            "unsupported" => "NATIVE_TOOL_UNSUPPORTED",
            "authorization_required" => "AUTHENTICATION_REQUIRED",
            "temporary_failure" => "NATIVE_TOOL_TEMPORARILY_UNAVAILABLE",
            _ => return Ok(()),
        };
        Err(format!(
            "{code}: 此连接的模型 {} 在 {} 操作中最近验证失败（{}）；尚未发出新请求。请检查任务模型设置与账号，连接或凭据改变后重新验证；原连接可在 {} 秒后重新检查。",
            entry.model,
            entry.operation,
            entry.code.as_deref().unwrap_or("UNVERIFIED"),
            60 - (at - entry.checked_at)
        ))
    }
    async fn observe_at(
        &self,
        ticket: &Ticket,
        state: &str,
        code: Option<&str>,
        at: u64,
    ) -> Result<(), String> {
        if !matches!(
            state,
            "ready" | "authorization_required" | "unsupported" | "temporary_failure" | "unverified"
        ) {
            return Err("invalid capability observation".into());
        }
        let _write = self.writes.lock().await;
        let bytes = {
            let mut store = self.state.lock();
            if store
                .entries
                .get(&ticket.key)
                .is_some_and(|old| old.sequence > ticket.sequence)
            {
                return Ok(());
            }
            store.entries.insert(
                ticket.key.clone(),
                Observation {
                    scope: ticket.scope.clone(),
                    role: ticket.role.clone(),
                    model: ticket.model.clone(),
                    driver: ticket.driver.clone(),
                    operation: ticket.operation.clone(),
                    state: state.into(),
                    code: code.map(|value| value.chars().take(80).collect()),
                    checked_at: at,
                    sequence: ticket.sequence,
                },
            );
            while store.entries.len() > MAX_ENTRIES {
                let oldest = store
                    .entries
                    .iter()
                    .min_by_key(|(_, o)| o.sequence)
                    .map(|(key, _)| key.clone())
                    .unwrap();
                store.entries.remove(&oldest);
            }
            serde_json::to_vec(&*store).map_err(|e| e.to_string())?
        };
        if self.persistent && bytes.len() as u64 <= MAX_BYTES {
            dsh_atomic_write::write_file_atomic(
                &self.path,
                &bytes,
                dsh_atomic_write::WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn view(
        &self,
        provider: &str,
        profile: &Value,
        credential: Option<&str>,
        authorization: &str,
        role: &str,
    ) -> Value {
        self.view_at(provider, profile, credential, authorization, role, now())
    }
    fn view_at(
        &self,
        provider: &str,
        profile: &Value,
        credential: Option<&str>,
        authorization: &str,
        role: &str,
        at: u64,
    ) -> Value {
        let scope = self.scope(provider, profile, credential);
        let compatibility = crate::native_tool_compatibility::status(Some(profile));
        let configured = profile["baseURL"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            || profile["authProvider"].is_string();
        let state = if !configured {
            "unconfigured"
        } else if compatibility == "unsupported" {
            "unsupported"
        } else if matches!(authorization, "missing" | "invalid" | "expired") {
            "authorization_required"
        } else {
            "unverified"
        };
        let store = self.state.lock();
        let mut rows = store
            .entries
            .values()
            .filter(|entry| entry.scope == scope && entry.role == role)
            .collect::<Vec<_>>();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.sequence));
        let observations=rows.into_iter().take(16).map(|entry|{
            let ttl=if entry.state=="ready"{86400}else{60};
            let current=at>=entry.checked_at&&at-entry.checked_at<ttl;
            json!({"model":entry.model,"driverModel":entry.driver,"operation":entry.operation,"state":if current{entry.state.as_str()}else{"unverified"},"lastState":entry.state,"code":entry.code,"checkedAt":entry.checked_at,"expiresAt":entry.checked_at.saturating_add(ttl),"expired":!current})
        }).collect::<Vec<_>>();
        json!({"registered":true,"configured":configured,"compatibility":compatibility,"authorization":authorization,"state":state,"modelState":"per_observation","permissions":"checked_per_call","observations":observations})
    }
}

pub(super) fn http_state(status: u16) -> Option<(&'static str, &'static str)> {
    match status {
        401 | 403 => Some(("authorization_required", "HTTP_AUTH_REJECTED")),
        404 | 405 | 501 => Some(("unsupported", "HTTP_ENDPOINT_OR_MODEL_UNSUPPORTED")),
        408 | 429 | 500..=599 => Some(("temporary_failure", "HTTP_TEMPORARY_FAILURE")),
        _ => None, // A malformed prompt/mask is not evidence that the model is unavailable.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn proof_is_credential_model_operation_scoped_expires_and_survives_restart() {
        let root = std::env::temp_dir().join(format!("native-capability-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = NativeCapabilities::open(&root);
        let profile = json!({"baseURL":"https://example.test/v1","api":"openai-responses"});
        let old = store.ticket(
            "provider",
            &profile,
            Some("secret-one"),
            "image",
            "model",
            Some("driver"),
            "generate",
        );
        let fresh = store.ticket(
            "provider",
            &profile,
            Some("secret-one"),
            "image",
            "model",
            Some("driver"),
            "generate",
        );
        store.observe_at(&fresh, "ready", None, 100).await.unwrap();
        store
            .observe_at(&old, "temporary_failure", Some("timeout"), 101)
            .await
            .unwrap();
        let view = store.view_at(
            "provider",
            &profile,
            Some("secret-one"),
            "present",
            "image",
            102,
        );
        assert_eq!(view["observations"][0]["state"], "ready");
        assert_eq!(
            view["state"], "unverified",
            "a connection-wide claim must not hide the exact tested model/operation"
        );
        assert!(
            store.view_at(
                "provider",
                &profile,
                Some("rotated"),
                "present",
                "image",
                102
            )["observations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.view_at(
                "provider",
                &profile,
                Some("secret-one"),
                "present",
                "image",
                86_500
            )["observations"][0]["state"],
            "unverified"
        );
        let loaded = NativeCapabilities::open(&root);
        assert_eq!(
            loaded.view_at(
                "provider",
                &profile,
                Some("secret-one"),
                "present",
                "image",
                103
            )["observations"][0]["state"],
            "ready"
        );
        let bytes = std::fs::read_to_string(&loaded.path).unwrap();
        assert!(!bytes.contains("secret-one"));
        assert!(!view.to_string().contains("secret-one"));
        assert_eq!(http_state(400), None);
        assert_eq!(http_state(403).unwrap().0, "authorization_required");
        assert_eq!(http_state(429).unwrap().0, "temporary_failure");
        store
            .observe_at(
                &fresh,
                "unsupported",
                Some("HTTP_ENDPOINT_OR_MODEL_UNSUPPORTED"),
                200,
            )
            .await
            .unwrap();
        assert!(
            store
                .preflight_at(&fresh, 201)
                .unwrap_err()
                .starts_with("NATIVE_TOOL_UNSUPPORTED:")
        );
        assert!(store.preflight_at(&fresh, 260).is_ok());
        let edit = store.ticket(
            "provider",
            &profile,
            Some("secret-one"),
            "image",
            "model",
            Some("driver"),
            "edit",
        );
        assert!(
            store.preflight_at(&edit, 201).is_ok(),
            "a generate failure cannot disable a different operation"
        );
        let changed = store.ticket(
            "provider",
            &profile,
            Some("secret-two"),
            "image",
            "model",
            Some("driver"),
            "generate",
        );
        assert!(store.preflight_at(&changed, 201).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
