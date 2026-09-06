use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use parking_lot::Mutex as SyncMutex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::adapter::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, AdapterScreenshot,
    ComputerUseAdapter,
};

const MAX_INLINE_SCREENSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_STDOUT_BYTES: usize = 24 * 1024 * 1024;
const MAX_COMMAND_STDERR_BYTES: usize = 1024 * 1024;
const DEFAULT_SESSION: &str = "default";

pub struct CommandAdapter {
    command: String,
    /// Legacy command adapters are process-per-action, so the Host keeps the
    /// persistent-session projection needed for owner cleanup and proxy
    /// retirement. `operations` closes the start/owner-dispose race.
    operations: Mutex<()>,
    owner_sessions: SyncMutex<HashMap<String, HashSet<String>>>,
    timeout: Duration,
}

impl CommandAdapter {
    pub fn new(command: impl Into<String>) -> Result<Self, AdapterError> {
        Self::with_timeout(command, Duration::from_secs(60))
    }

    pub fn with_timeout(
        command: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AdapterError> {
        let command = command.into();
        if command.trim().is_empty() {
            return Err(AdapterError::new(
                "COMPUTER_USE_COMMAND_REQUIRED",
                "command adapter requires a non-empty executable path",
            ));
        }
        if timeout < Duration::from_secs(1) || timeout > Duration::from_secs(300) {
            return Err(AdapterError::new(
                "COMPUTER_USE_TIMEOUT",
                "command adapter timeout must be between 1 and 300 seconds",
            ));
        }
        Ok(Self {
            command,
            operations: Mutex::new(()),
            owner_sessions: SyncMutex::new(HashMap::new()),
            timeout,
        })
    }

    fn owner_id<'a>(&self, request: &'a AdapterRequest) -> &'a str {
        request.owner_id.as_deref().unwrap_or("host")
    }

    fn client_session_id(arguments: &Value) -> Result<String, AdapterError> {
        match arguments.get("sessionId") {
            None => Ok(DEFAULT_SESSION.to_string()),
            Some(Value::String(value)) if !value.is_empty() && value.len() <= 256 => {
                Ok(value.clone())
            }
            _ => Err(AdapterError::new(
                "COMPUTER_USE_SESSION_ID",
                "command adapter sessionId must be a non-empty string of at most 256 bytes",
            )),
        }
    }

    /// Keep argv (`<action> <json>`) compatible with the original adapter,
    /// while binding every persistent session to a Host-owned owner. Existing
    /// adapters that key storage by sessionId gain isolation automatically;
    /// v2-aware adapters can additionally use ownerId and clientSessionId.
    fn wire_arguments(
        &self,
        request: &AdapterRequest,
    ) -> Result<(Value, String, String, String), AdapterError> {
        let owner_id = self.owner_id(request).to_string();
        let client_session_id = Self::client_session_id(&request.arguments)?;
        let wire_session_id = if request.owner_id.is_none() {
            // Preserve the old direct/host invocation exactly. Agent and GUI
            // requests always carry an owner and use the isolated v2 id.
            client_session_id.clone()
        } else {
            let digest = format!(
                "{:x}",
                Sha256::digest(format!("{owner_id}\0{client_session_id}").as_bytes())
            );
            format!("dsh-{}", &digest[..32])
        };
        let mut object = request.arguments.as_object().cloned().ok_or_else(|| {
            AdapterError::new(
                "COMPUTER_USE_INVALID_ARGUMENT",
                "command adapter arguments must be a JSON object",
            )
        })?;
        // These values overwrite any model-supplied keys. They can only be
        // derived from the trusted ToolRunContext/Host GUI owner.
        object.insert("dshProtocolVersion".to_string(), Value::from(2));
        object.insert("ownerId".to_string(), Value::String(owner_id.clone()));
        object.insert(
            "clientSessionId".to_string(),
            Value::String(client_session_id.clone()),
        );
        object.insert(
            "sessionId".to_string(),
            Value::String(wire_session_id.clone()),
        );
        Ok((
            Value::Object(object),
            owner_id,
            client_session_id,
            wire_session_id,
        ))
    }

    async fn run(
        &self,
        action: &str,
        arguments: &Value,
        signal: AbortPredicate,
    ) -> Result<Value, AdapterError> {
        let serialized = serde_json::to_string(arguments)
            .map_err(|error| AdapterError::new("COMPUTER_USE_SERIALIZE", error.to_string()))?;
        let output = dsh_native_command::run_native_command_bounded(
            &self.command,
            &[action.to_string(), serialized],
            Some(signal),
            dsh_native_command::NativeCommandLimits {
                timeout: self.timeout,
                stdout_bytes: MAX_COMMAND_STDOUT_BYTES,
                stderr_bytes: MAX_COMMAND_STDERR_BYTES,
            },
        )
        .await
        .map_err(|error| {
            AdapterError::new(
                error
                    .code
                    .as_deref()
                    .map(|code| format!("COMPUTER_USE_COMMAND_{code}"))
                    .unwrap_or_else(|| "COMPUTER_USE_COMMAND_FAILED".to_string()),
                format!(
                    "computer-use adapter failed: {}{}",
                    error,
                    if error.stderr.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", error.stderr.trim())
                    }
                ),
            )
        })?;
        serde_json::from_str(output.stdout.trim()).map_err(|error| {
            AdapterError::new(
                "COMPUTER_USE_COMMAND_INVALID_JSON",
                format!("computer-use adapter returned invalid JSON: {error}"),
            )
        })
    }

    fn update_activity(&self, owner_id: &str, session_id: &str, action: &str, value: &Value) {
        if !value.is_object() || value.get("ok").and_then(Value::as_bool) == Some(false) {
            return;
        }
        let reported = value.get("sessionActive").and_then(Value::as_bool);
        let opens = matches!(
            action,
            "start"
                | "navigate"
                | "status"
                | "cua_browser_state"
                | "capture"
                | "click"
                | "double_click"
                | "type"
                | "input"
                | "scroll"
        );
        let closes = action == "close";
        let mut owners = self.owner_sessions.lock();
        if reported == Some(true) || (reported.is_none() && opens) {
            owners
                .entry(owner_id.to_string())
                .or_default()
                .insert(session_id.to_string());
        } else if reported == Some(false) || closes {
            if let Some(sessions) = owners.get_mut(owner_id) {
                sessions.remove(session_id);
                if sessions.is_empty() {
                    owners.remove(owner_id);
                }
            }
        }
    }

    fn rewrite_response_ids(
        &self,
        value: &mut Value,
        owner_id: &str,
        client_session_id: &str,
        wire_session_id: &str,
        action: &str,
    ) {
        let Some(object) = value.as_object_mut() else {
            return;
        };
        if object.get("sessionId").and_then(Value::as_str) == Some(wire_session_id) {
            object.insert(
                "sessionId".to_string(),
                Value::String(client_session_id.to_string()),
            );
        }
        let mut lookup = HashMap::new();
        for client in self
            .owner_sessions
            .lock()
            .get(owner_id)
            .into_iter()
            .flatten()
        {
            let digest = format!(
                "{:x}",
                Sha256::digest(format!("{owner_id}\0{client}").as_bytes())
            );
            lookup.insert(format!("dsh-{}", &digest[..32]), client.clone());
            lookup.insert(client.clone(), client.clone());
        }
        if action == "list_sessions" {
            let filtered = object
                .remove("sessions")
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|mut session| {
                    if let Some(wire) = session.as_str() {
                        return lookup.get(wire).cloned().map(Value::String);
                    }
                    let row = session.as_object_mut()?;
                    let wire = row.get("sessionId")?.as_str()?.to_string();
                    let client = lookup.get(&wire)?.clone();
                    row.insert("sessionId".to_string(), Value::String(client));
                    row.remove("ownerId");
                    Some(session)
                })
                .collect();
            object.insert("sessions".to_string(), Value::Array(filtered));
        } else if let Some(sessions) = object.get_mut("sessions").and_then(Value::as_array_mut) {
            for session in sessions {
                if let Some(client) = session.as_str().and_then(|wire| lookup.get(wire)) {
                    *session = Value::String(client.clone());
                }
            }
        }
        // Never let an adapter echo a forged owner boundary back as protocol
        // state. The authoritative owner remains inside the Host.
        object.remove("ownerId");
    }

    async fn close_tracked(
        &self,
        owner_id: &str,
        sessions: impl IntoIterator<Item = String>,
    ) -> (Option<AdapterError>, HashSet<String>) {
        let mut first_error = None;
        let mut failed = HashSet::new();
        for client_session_id in sessions {
            let request = AdapterRequest {
                action: "close".to_string(),
                arguments: serde_json::json!({
                    "action": "close",
                    "sessionId": client_session_id
                }),
                owner_id: Some(owner_id.to_string()),
            };
            let result = match self.wire_arguments(&request) {
                Ok((arguments, _, _, _)) => self
                    .run("close", &arguments, Arc::new(|| false))
                    .await
                    .and_then(Self::validate_cleanup_output),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                failed.insert(client_session_id);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        (first_error, failed)
    }

    fn validate_cleanup_output(value: Value) -> Result<(), AdapterError> {
        let Some(object) = value.as_object() else {
            return Err(AdapterError::new(
                "COMPUTER_USE_COMMAND_INVALID_OUTPUT",
                "computer-use close returned a non-object JSON value",
            ));
        };
        if object.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(AdapterError::new(
                "COMPUTER_USE_COMMAND_CLOSE_REJECTED",
                object
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("computer-use adapter rejected owner cleanup"),
            ));
        }
        Ok(())
    }
}

fn extract_screenshot(value: &mut Value) -> Result<Option<AdapterScreenshot>, AdapterError> {
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    let Some(candidate) = object.get("screenshot") else {
        return Ok(None);
    };
    let Some(encoded) = candidate.get("base64").and_then(Value::as_str) else {
        return Ok(None);
    };
    if encoded.len() > (MAX_INLINE_SCREENSHOT_BYTES * 4 / 3) + 8 {
        return Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE",
            "adapter screenshot exceeds the 16 MiB limit",
        ));
    }
    let media_type = candidate
        .get("mediaType")
        .and_then(Value::as_str)
        .unwrap_or("image/png")
        .to_string();
    if !matches!(
        media_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TYPE",
            format!("unsupported adapter screenshot media type: {media_type}"),
        ));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            AdapterError::new(
                "COMPUTER_USE_SCREENSHOT_ENCODING",
                format!("adapter screenshot is not valid base64: {error}"),
            )
        })?;
    if data.len() > MAX_INLINE_SCREENSHOT_BYTES {
        return Err(AdapterError::new(
            "COMPUTER_USE_SCREENSHOT_TOO_LARGE",
            "adapter screenshot exceeds the 16 MiB limit",
        ));
    }
    let name = candidate
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    object.remove("screenshot");
    Ok(Some(AdapterScreenshot {
        data,
        media_type,
        name,
    }))
}

#[async_trait]
impl ComputerUseAdapter for CommandAdapter {
    fn adapter_id(&self) -> &'static str {
        "command"
    }

    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        let _operation = self.operations.lock().await;
        let action = request.action.clone();
        let (arguments, owner_id, client_session_id, wire_session_id) =
            self.wire_arguments(&request)?;
        let mut value = self.run(&action, &arguments, signal).await?;
        self.update_activity(&owner_id, &client_session_id, &action, &value);
        self.rewrite_response_ids(
            &mut value,
            &owner_id,
            &client_session_id,
            &wire_session_id,
            &action,
        );
        let screenshot = extract_screenshot(&mut value)?;
        Ok(AdapterOutput { value, screenshot })
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        let _operation = self.operations.lock().await;
        let owners = std::mem::take(&mut *self.owner_sessions.lock());
        let mut first_error = None;
        for (owner_id, sessions) in owners {
            let (error, _) = self.close_tracked(&owner_id, sessions).await;
            if let Some(error) = error
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn close_owner(&self, owner_id: &str) -> Result<(), AdapterError> {
        let _operation = self.operations.lock().await;
        let sessions = self
            .owner_sessions
            .lock()
            .remove(owner_id)
            .unwrap_or_default();
        let (error, failed) = self.close_tracked(owner_id, sessions).await;
        if !failed.is_empty() {
            self.owner_sessions
                .lock()
                .entry(owner_id.to_string())
                .or_default()
                .extend(failed);
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn has_owner_activity(&self, owner_id: &str) -> bool {
        self.owner_sessions
            .lock()
            .get(owner_id)
            .is_some_and(|sessions| !sessions.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_inline_screenshot_without_leaking_base64_to_model_text() {
        let mut value = serde_json::json!({
            "ok": true,
            "screenshot": {
                "base64": "iVBORw0KGgo=",
                "mediaType": "image/png",
                "name": "remote.png"
            }
        });
        let screenshot = extract_screenshot(&mut value).unwrap().unwrap();
        assert_eq!(screenshot.data, b"\x89PNG\r\n\x1a\n");
        assert_eq!(screenshot.name.as_deref(), Some("remote.png"));
        assert!(value.get("screenshot").is_none());
    }

    #[test]
    fn wire_protocol_overrides_forged_owner_and_namespaces_sessions() {
        let adapter = CommandAdapter::new("fixture").unwrap();
        let request = AdapterRequest::from_arguments(&serde_json::json!({
            "action": "start",
            "sessionId": "default",
            "ownerId": "model-forged"
        }))
        .unwrap()
        .with_owner_id("conversation-a");
        let (wire, owner, client, session) = adapter.wire_arguments(&request).unwrap();
        assert_eq!(owner, "conversation-a");
        assert_eq!(client, "default");
        assert_eq!(wire["dshProtocolVersion"], 2);
        assert_eq!(wire["ownerId"], "conversation-a");
        assert_eq!(wire["clientSessionId"], "default");
        assert_eq!(wire["sessionId"], session);
        assert_ne!(session, "default");

        let other = AdapterRequest::from_arguments(&serde_json::json!({
            "action": "start",
            "sessionId": "default"
        }))
        .unwrap()
        .with_owner_id("conversation-b");
        assert_ne!(session, adapter.wire_arguments(&other).unwrap().3);
    }

    #[test]
    fn direct_legacy_invocation_keeps_original_session_id() {
        let adapter = CommandAdapter::new("fixture").unwrap();
        let request = AdapterRequest::from_arguments(&serde_json::json!({
            "action": "capture",
            "sessionId": "legacy-session"
        }))
        .unwrap();
        let (wire, owner, client, session) = adapter.wire_arguments(&request).unwrap();
        assert_eq!(owner, "host");
        assert_eq!(client, "legacy-session");
        assert_eq!(session, "legacy-session");
        assert_eq!(wire["sessionId"], "legacy-session");
    }

    #[test]
    fn activity_projection_is_owner_scoped_and_closeable() {
        let adapter = CommandAdapter::new("fixture").unwrap();
        adapter.update_activity(
            "conversation-a",
            "default",
            "start",
            &serde_json::json!({"ok": true}),
        );
        adapter.update_activity(
            "conversation-b",
            "default",
            "capture",
            &serde_json::json!({"ok": true}),
        );
        assert!(adapter.has_owner_activity("conversation-a"));
        assert!(adapter.has_owner_activity("conversation-b"));
        adapter.update_activity(
            "conversation-a",
            "default",
            "close",
            &serde_json::json!({"ok": true}),
        );
        assert!(!adapter.has_owner_activity("conversation-a"));
        assert!(adapter.has_owner_activity("conversation-b"));
    }

    #[test]
    fn list_sessions_filters_unknown_and_other_owner_rows() {
        let adapter = CommandAdapter::new("fixture").unwrap();
        for (owner, session) in [
            ("conversation-a", "alpha"),
            ("conversation-a", "beta"),
            ("conversation-b", "secret"),
        ] {
            adapter.update_activity(owner, session, "start", &serde_json::json!({"ok": true}));
        }
        let request = AdapterRequest::from_arguments(&serde_json::json!({
            "action": "list_sessions"
        }))
        .unwrap()
        .with_owner_id("conversation-a");
        let (_, _, _, alpha_wire) = adapter
            .wire_arguments(
                &AdapterRequest::from_arguments(&serde_json::json!({
                    "action": "capture",
                    "sessionId": "alpha"
                }))
                .unwrap()
                .with_owner_id("conversation-a"),
            )
            .unwrap();
        let (_, _, _, beta_wire) = adapter
            .wire_arguments(
                &AdapterRequest::from_arguments(&serde_json::json!({
                    "action": "capture",
                    "sessionId": "beta"
                }))
                .unwrap()
                .with_owner_id("conversation-a"),
            )
            .unwrap();
        let (_, _, _, secret_wire) = adapter
            .wire_arguments(
                &AdapterRequest::from_arguments(&serde_json::json!({
                    "action": "capture",
                    "sessionId": "secret"
                }))
                .unwrap()
                .with_owner_id("conversation-b"),
            )
            .unwrap();
        let (_, owner, client, wire) = adapter.wire_arguments(&request).unwrap();
        let mut value = serde_json::json!({
            "ok": true,
            "sessions": [
                alpha_wire,
                {"sessionId": beta_wire, "title": "kept", "ownerId": "echo"},
                secret_wire,
                "unknown",
                {"title": "missing id"}
            ]
        });
        adapter.rewrite_response_ids(&mut value, &owner, &client, &wire, "list_sessions");
        assert_eq!(
            value["sessions"],
            serde_json::json!([
                "alpha",
                {"sessionId": "beta", "title": "kept"}
            ])
        );
    }

    #[tokio::test]
    async fn failed_owner_cleanup_keeps_activity_for_retry() {
        let missing = std::env::temp_dir()
            .join(format!(
                "missing-computer-use-adapter-{}",
                uuid::Uuid::new_v4()
            ))
            .to_string_lossy()
            .into_owned();
        let adapter = CommandAdapter::new(missing).unwrap();
        adapter.update_activity(
            "conversation-a",
            "default",
            "start",
            &serde_json::json!({"ok": true}),
        );
        assert!(adapter.close_owner("conversation-a").await.is_err());
        assert!(adapter.has_owner_activity("conversation-a"));
    }

    #[test]
    fn protocol_level_cleanup_rejection_is_not_treated_as_closed() {
        let failure = CommandAdapter::validate_cleanup_output(serde_json::json!({
            "ok": false,
            "message": "remote session is busy"
        }))
        .unwrap_err();
        assert_eq!(failure.code, "COMPUTER_USE_COMMAND_CLOSE_REJECTED");
        assert_eq!(failure.message, "remote session is busy");
        assert!(
            CommandAdapter::validate_cleanup_output(serde_json::json!({
                "ok": true,
                "closed": false
            }))
            .is_ok()
        );
    }

    #[test]
    fn bounded_command_transport_can_carry_the_largest_allowed_screenshot() {
        let encoded_screenshot = (MAX_INLINE_SCREENSHOT_BYTES * 4 / 3) + 8;
        assert!(MAX_COMMAND_STDOUT_BYTES > encoded_screenshot);
        assert_eq!(MAX_COMMAND_STDERR_BYTES, 1024 * 1024);
        assert!(CommandAdapter::with_timeout("fixture", Duration::from_millis(999)).is_err());
        assert!(CommandAdapter::with_timeout("fixture", Duration::from_secs(300)).is_ok());
    }
}
