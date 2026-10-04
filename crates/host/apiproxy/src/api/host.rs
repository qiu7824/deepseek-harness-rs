//! `host` domain contract: host-level unary methods. No protocol version:
//! client and host ship together. Rust port of
//! `packages/host/apiproxy/src/api/host.ts`.
//!
//! # Deviations
//!
//! - `DirectoryEntry`/`DirectoryListing` reuse the directory-picker seam
//!   crate's wire types (identical shape, camelCase serde); the TS api/
//!   layer redeclares them because it cannot import the seam package, while
//!   the Rust contract layer can share one definition.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub use dsh_host_directory_picker::{DirectoryEntry, DirectoryListing};

use crate::api::rpc::{RpcRequest, RpcResponse};
use crate::fetch::handler::AbortSignal;

/// Identity of this process, shared by `host.describe` and desktop readiness.
/// The nonce distinguishes a new Host when an operating system reuses a PID.
/// It is an ownership check for local launchers, not an authentication secret.
pub fn host_process_identity() -> (u32, String) {
    static INSTANCE_ID: OnceLock<String> = OnceLock::new();
    (
        std::process::id(),
        INSTANCE_ID
            .get_or_init(|| uuid::Uuid::new_v4().to_string())
            .clone(),
    )
}

/// `host.describe` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDescribeResult {
    /// Resolved Harness data home used by the running Host.
    pub home: String,
    /// The host app's package version.
    pub version: String,
    /// The host process working directory.
    pub cwd: String,
    /// Optional for compatibility with older Hosts and externally configured
    /// deployments. Desktop-owned Hosts expose both identity fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Defaults applied when a new agent doesn't specify them explicitly;
    /// absent when the host configures no explicit default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Count of currently attached sessions (those with a live agent).
    pub attached_sessions: u64,
    /// Whether this deployment can hand a path to a user-visible native
    /// desktop.
    pub can_open_path: bool,
    /// User todo snapshots written outside a turn can be restored safely.
    /// Older hosts omit this field; clients must not assume support by version.
    #[serde(default)]
    pub supports_idle_todo_edits: bool,
}

#[cfg(test)]
mod compatibility_tests {
    use super::{HostDescribeResult, host_process_identity};

    #[test]
    fn legacy_host_description_does_not_advertise_idle_todo_edits() {
        let legacy = serde_json::json!({
            "home": "home", "version": "0.1", "cwd": ".",
            "attachedSessions": 0, "canOpenPath": false
        });
        let mut description: HostDescribeResult = serde_json::from_value(legacy).unwrap();
        assert!(!description.supports_idle_todo_edits);
        assert_eq!(description.process_id, None);
        assert_eq!(description.instance_id, None);
        description.supports_idle_todo_edits = true;
        let (pid, instance_id) = host_process_identity();
        description.process_id = Some(pid);
        description.instance_id = Some(instance_id.clone());
        let encoded = serde_json::to_value(description).unwrap();
        assert_eq!(encoded["supportsIdleTodoEdits"], true);
        assert_eq!(encoded["processId"], pid);
        assert_eq!(encoded["instanceId"], instance_id);
    }

    #[test]
    fn process_identity_is_stable_and_has_a_uuid_nonce() {
        let identity = host_process_identity();
        assert_eq!(identity.0, std::process::id());
        assert_eq!(host_process_identity(), identity);
        assert_eq!(
            uuid::Uuid::parse_str(&identity.1)
                .unwrap()
                .get_version_num(),
            4
        );
    }
}

/// `host.pickDirectory` response value (null = operator cancelled).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPickDirectoryResult {
    pub path: Option<String>,
}

/// `host.listDirectory` request payload (absent lists the home directory).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostListDirectoryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `host.createDirectory` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCreateDirectoryRequest {
    pub path: String,
    pub name: String,
}

/// `host.createDirectory` response value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCreateDirectoryResult {
    pub path: String,
}

/// `host.openPath` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostOpenPathRequest {
    pub path: String,
}

/// `host.openPath` response value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOpenPathResult {
    pub opened: bool,
}

/// Host-level unary methods.
#[async_trait]
pub trait HostApi: Send + Sync {
    /// One-shot host snapshot. Empty payload uses the literal `{}`.
    async fn describe(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<HostDescribeResult>;

    /// Open the operating system's single-directory picker; cancellation
    /// returns null. Only served under the `native` capability.
    async fn pick_directory(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<HostPickDirectoryResult>;

    /// List one directory level for the in-app browser. Only served under
    /// the `browse` capability; unreadable or missing targets fail with
    /// `directory-unreadable`.
    async fn list_directory(
        &self,
        request: RpcRequest<HostListDirectoryRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<DirectoryListing>;

    /// Create one child directory under an existing parent. Only served
    /// under the `browse` capability.
    async fn create_directory(
        &self,
        request: RpcRequest<HostCreateDirectoryRequest>,
    ) -> RpcResponse<HostCreateDirectoryResult>;

    /// Open a filesystem path with the operating system's default
    /// application.
    async fn open_path(
        &self,
        request: RpcRequest<HostOpenPathRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<HostOpenPathResult>;
}
