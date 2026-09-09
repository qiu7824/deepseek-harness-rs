use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

pub type AbortPredicate = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlOrigin {
    #[default]
    Agent,
    Human,
}

#[derive(Clone)]
pub struct AdapterRequest {
    pub action: String,
    pub arguments: Value,
    /// Host-owned isolation scope. Model arguments cannot set this value.
    /// Built-in browser sessions use the owning agent/session id.
    pub owner_id: Option<String>,
    /// Assigned by the Host transport, never deserialized from tool arguments.
    pub origin: ControlOrigin,
}

impl AdapterRequest {
    pub fn from_arguments(arguments: &Value) -> Result<Self, AdapterError> {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AdapterError::new(
                    "COMPUTER_USE_INVALID_ARGUMENT",
                    "computer_use.action must be a non-empty string",
                )
            })?
            .to_string();
        Ok(Self {
            action,
            arguments: arguments.clone(),
            owner_id: None,
            origin: ControlOrigin::Agent,
        })
    }

    pub fn with_owner_id(mut self, owner_id: impl Into<String>) -> Self {
        self.owner_id = Some(owner_id.into());
        self
    }

    pub fn with_origin(mut self, origin: ControlOrigin) -> Self {
        self.origin = origin;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterScreenshot {
    pub data: Vec<u8>,
    pub media_type: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdapterOutput {
    pub value: Value,
    pub screenshot: Option<AdapterScreenshot>,
}

impl AdapterOutput {
    pub fn json(value: Value) -> Self {
        Self {
            value,
            screenshot: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub code: String,
    pub message: String,
}

impl AdapterError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new("COMPUTER_USE_ABORTED", "computer-use action was cancelled")
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AdapterError {}

/// Stable boundary for browser, desktop, command and future remote-device
/// controllers. An adapter owns its persistent execution environment and
/// returns pixels separately from JSON so the Host can validate and store
/// them as an image attachment.
#[async_trait]
pub trait ComputerUseAdapter: Send + Sync + 'static {
    fn adapter_id(&self) -> &'static str;

    /// Browsers own isolated sessions; desktop drivers override this with a
    /// physical device identity so two conversations cannot race one desktop.
    fn control_scope(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        let name = request
            .arguments
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("default");
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err(AdapterError::new(
                "COMPUTER_USE_SESSION_ID",
                "invalid control session name",
            ));
        }
        Ok(format!(
            "{}\0{}",
            request.owner_id.as_deref().unwrap_or("host"),
            name
        ))
    }

    async fn change_control(
        &self,
        _request: &AdapterRequest,
        _manual: bool,
    ) -> Result<(), AdapterError> {
        Ok(())
    }

    /// A side-effect-light readiness probe for settings and GUI diagnostics.
    /// Implementations must not launch a controlled session here.
    fn availability(&self) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn execute(
        &self,
        request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError>;

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn close_owner(&self, _owner_id: &str) -> Result<(), AdapterError> {
        Ok(())
    }

    fn has_owner_activity(&self, _owner_id: &str) -> bool {
        false
    }

    /// Count the owning agent's running turn as activity for an existing,
    /// ready connection. This must be local bookkeeping only: no launch,
    /// transport heartbeat, or control-ownership changes.
    fn mark_owner_active(&self, _owner_id: &str) {}

    /// Remove persistent sessions whose backing process disappeared or idle
    /// lifetime elapsed, and return owners that transitioned to idle.
    async fn reap_inactive(&self) -> Vec<String> {
        Vec::new()
    }
}
