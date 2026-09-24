//! Application permissions are independent of filesystem permissions. Targets
//! are observed by the adapter and never deserialized from model arguments.
use crate::{
    AbortPredicate, AdapterError, AdapterOutput, AdapterRequest, ComputerUseAdapter, ControlOrigin,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComputerTargetIdentity {
    pub host_id: String,
    pub device_id: String,
    pub application_id: String,
    pub application_revision: String,
    pub origin: Option<String>,
    /// Ephemeral window/process/CDP target generation, rechecked at dispatch.
    pub target_revision: String,
    pub label: String,
}

#[derive(Clone)]
pub struct ComputerPermissionRequest {
    pub owner_id: String,
    pub target: ComputerTargetIdentity,
    pub action: String,
    pub scopes: Vec<String>,
    pub signal: AbortPredicate,
}
pub struct ComputerPermissionLease {
    pub target: ComputerTargetIdentity,
    pub revision: u64,
    /// True while the authorization is still live.
    pub valid: AbortPredicate,
}
pub trait ComputerPermissionService: Send + Sync + 'static {
    fn authorize(
        &self,
        request: ComputerPermissionRequest,
    ) -> futures::future::BoxFuture<'static, Result<ComputerPermissionLease, AdapterError>>;
}
impl cordis::Service for dyn ComputerPermissionService {
    fn service_name(&self) -> &'static str {
        "computerPermissions"
    }
}

pub const COMPUTER_PERMISSION_SCOPES: &[&str] = &[
    "screen_read",
    "input",
    "clipboard_read",
    "clipboard_write",
    "file_upload",
    "external_actions",
    "launch",
];

pub(crate) fn executable_revision(path: &std::path::Path) -> Result<String, AdapterError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| {
        AdapterError::new("COMPUTER_USE_IDENTITY_UNAVAILABLE", error.to_string())
    })?;
    if file
        .metadata()
        .map_err(|error| AdapterError::new("COMPUTER_USE_IDENTITY_UNAVAILABLE", error.to_string()))?
        .len()
        > 512 * 1024 * 1024
    {
        return Err(AdapterError::new(
            "COMPUTER_USE_IDENTITY_UNAVAILABLE",
            "Executable exceeds identity budget",
        ));
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let count = file.read(&mut bytes).map_err(|error| {
            AdapterError::new("COMPUTER_USE_IDENTITY_UNAVAILABLE", error.to_string())
        })?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
pub(crate) fn browser_origin(url: &str) -> Result<String, AdapterError> {
    if url == "about:blank" {
        return Ok("about:blank".into());
    }
    let url = reqwest::Url::parse(url).map_err(|_| {
        AdapterError::new(
            "COMPUTER_USE_IDENTITY_UNAVAILABLE",
            "Cannot attest browser origin",
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AdapterError::new(
            "COMPUTER_USE_IDENTITY_UNAVAILABLE",
            "Origin is not an eligible web origin",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

pub fn action_scopes(action: &str) -> Vec<String> {
    let scopes: &[&str] = match action {
        "close" | "takeover" | "resume_agent" | "release_inputs" => &[],
        "clipboard_read" => &["clipboard_read"],
        "clipboard_write" => &["clipboard_write"],
        "upload_files" => &["screen_read", "input", "file_upload", "external_actions"],
        "launch_app" => &["launch"],
        "status" | "capture" | "video_info" | "video_frame" | "ax_state" | "cua_browser_state"
        | "list_apps" | "list_windows" | "list_sessions" | "list_tabs" => &["screen_read"],
        "start" => &["screen_read", "launch"],
        // Generic input can submit or publish. Without application semantics
        // it cannot safely be advertised as excluding external effects.
        _ => &["screen_read", "input", "external_actions"],
    };
    scopes.iter().map(|scope| scope.to_string()).collect()
}

pub(crate) struct PermissionedAdapter {
    inner: Arc<dyn ComputerUseAdapter>,
    service: Arc<dyn ComputerPermissionService>,
}
impl PermissionedAdapter {
    pub fn new(
        inner: Arc<dyn ComputerUseAdapter>,
        service: Arc<dyn ComputerPermissionService>,
    ) -> Self {
        Self { inner, service }
    }
}

#[async_trait]
impl ComputerUseAdapter for PermissionedAdapter {
    fn adapter_id(&self) -> &'static str {
        self.inner.adapter_id()
    }
    fn adapter_id_for(&self, args: &Value) -> Result<&'static str, AdapterError> {
        self.inner.adapter_id_for(args)
    }
    fn targets(&self) -> Value {
        self.inner.targets()
    }
    fn availability(&self) -> Result<(), AdapterError> {
        self.inner.availability()
    }
    fn availability_for(&self, args: &Value) -> Result<(), AdapterError> {
        self.inner.availability_for(args)
    }
    fn control_scope(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        self.inner.control_scope(request)
    }
    async fn permission_identity(
        &self,
        request: &AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<ComputerTargetIdentity, AdapterError> {
        self.inner.permission_identity(request, signal).await
    }
    async fn change_control(
        &self,
        request: &AdapterRequest,
        manual: bool,
    ) -> Result<(), AdapterError> {
        self.inner.change_control(request, manual).await
    }
    async fn execute(
        &self,
        mut request: AdapterRequest,
        signal: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        let scopes = action_scopes(&request.action);
        if request.origin == ControlOrigin::Human || scopes.is_empty() {
            return self.inner.execute(request, signal).await;
        }
        if signal() {
            return Err(AdapterError::cancelled());
        }
        let identity = self
            .inner
            .permission_identity(&request, signal.clone())
            .await?;
        if request.permission_target.as_ref().is_some_and(|expected| expected.host_id!=identity.host_id||expected.device_id!=identity.device_id||expected.application_id!=identity.application_id||expected.application_revision!=identity.application_revision||expected.origin!=identity.origin||expected.target_revision!=identity.target_revision) {
            return Err(AdapterError::new("COMPUTER_USE_APP_IDENTITY_CHANGED", "Observed application identity changed before dispatch"));
        }
        let lease = self
            .service
            .authorize(ComputerPermissionRequest {
                owner_id: request.owner_id.clone().unwrap_or_default(),
                target: identity,
                action: request.action.clone(),
                scopes,
                signal: signal.clone(),
            })
            .await?;
        if !(lease.valid)() || signal() {
            return Err(AdapterError::new(
                "COMPUTER_USE_PERMISSION_REVOKED",
                "Application authorization changed before dispatch",
            ));
        }
        request.permission_target = Some(lease.target.clone());
        let valid = lease.valid.clone();
        let source = signal.clone();
        let guarded: AbortPredicate = Arc::new(move || source() || !valid());
        let mut output = self.inner.execute(request, guarded).await?;
        if !(lease.valid)() {
            return Err(AdapterError::new(
                "COMPUTER_USE_PERMISSION_REVOKED",
                "Application permission was revoked; further actions are blocked and the interrupted action may have effects",
            ));
        }
        if signal() {
            return Err(AdapterError::cancelled());
        }
        if let Some(object) = output.value.as_object_mut() {
            object.insert("permissionRevision".into(), lease.revision.into());
            object.insert("targetRevision".into(), lease.target.target_revision.into());
        }
        Ok(output)
    }
    async fn close_owner(&self, owner: &str) -> Result<(), AdapterError> {
        self.inner.close_owner(owner).await
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
    fn has_owner_activity(&self, owner: &str) -> bool {
        self.inner.has_owner_activity(owner)
    }
    fn mark_owner_active(&self, owner: &str) {
        self.inner.mark_owner_active(owner)
    }
    async fn reap_inactive(&self) -> Vec<String> {
        self.inner.reap_inactive().await
    }
}
