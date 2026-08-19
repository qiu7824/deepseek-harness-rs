//! The composition layer: `createApiProxy`'s Rust counterpart, wired onto
//! the [`ApiProxyCarrier`] trait. Rust port of
//! `packages/host/apiproxy/src/api-proxy.ts` (implemented domain by
//! domain; this file lands the service skeleton plus the `host.*` domain).
//!
//! # Deviations
//!
//! - Domains not yet wired answer `internal` errors naming the method; each
//!   domain lands in its own milestone and replaces that arm.
//! - The process-local bookkeeping (selection WeakMap, preset-switch and
//!   session-creation chains, pending approval/question maps, mux queues)
//!   arrives with the domains that use it.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::Context;
use dsh_agent::Agent;
use dsh_host_directory_picker::{
    AbortSignal as PickerAbort, DirectoryPicker, DirectoryPickerBrowseCapability,
    DirectoryPickerCapability, DirectoryPickerErrorCode, DirectoryPickerListError,
};
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::api::host::{
    HostCreateDirectoryRequest, HostCreateDirectoryResult, HostDescribeResult,
    HostListDirectoryRequest, HostOpenPathRequest, HostOpenPathResult, HostPickDirectoryResult,
};
use crate::api::rpc::{
    ClientResponse, EmptyDetails, RpcError, RpcErrorBody, RpcId, RpcRequest, RpcResponse, RpcResult,
};
use crate::api::sessions::ModelSelection;
use crate::fetch::handler::{
    AbortSignal, ApiProxyCarrier, Body, DownloadResponse, FrameRequest, SessionLogQuery,
};

/// The host app version reported by `host.describe` (the TS placeholder —
/// reads apps/cli's package version once the CLI lands).
pub const HOST_VERSION: &str = "0.0.1";

/// Composition inputs supplied by the host app (TS `ApiProxyDefaults`).
pub struct ApiProxyDefaults {
    /// The model selection a session starts from when its own log names
    /// none. Read on every access rather than captured.
    pub default_model_selection: Arc<dyn Fn() -> ModelSelection + Send + Sync>,
    /// Default project directory for new sessions whose create request
    /// carries no cwd.
    pub cwd: String,
    /// Native open-with-default-application; injectable for carrier tests.
    pub open_path: Option<
        Arc<dyn Fn(String, AbortSignal) -> BoxFuture<'static, Result<(), String>> + Send + Sync>,
    >,
    /// Whether handing a path to the native opener can work at all.
    pub can_open_path: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Validated DEFLATE level for session-log ZIP entries; defaults to 6.
    pub session_export_compression_level: u32,
    /// Maximum artifact size eligible for one cold blankness read.
    pub cold_blank_probe_max_bytes: usize,
}

impl Default for ApiProxyDefaults {
    fn default() -> Self {
        Self {
            default_model_selection: Arc::new(|| ModelSelection {
                provider: String::new(),
                model: String::new(),
                reasoning_effort: None,
            }),
            cwd: std::env::current_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            open_path: None,
            can_open_path: None,
            session_export_compression_level: 6,
            cold_blank_probe_max_bytes: 1024,
        }
    }
}

/// The composed `ctx.apiProxy` service.
pub struct ApiProxyService {
    ctx: Context,
    defaults: Arc<ApiProxyDefaults>,
    resolver: Arc<crate::agent_lookup::AgentResolver>,
    /// Per-session process-local model selections (the TS `selections`
    /// WeakMap; the logged-request tier arrives with the request-header
    /// milestone).
    selections: parking_lot::Mutex<
        std::collections::HashMap<dsh_session::SessionId, crate::api::sessions::ModelSelection>,
    >,
    /// Per-session preset-switch chains (the TS `presetSwitches` map): each
    /// select request serializes behind the previous one so a queued request
    /// re-reads blankness and the roster after earlier switches committed.
    /// The `u64` is a per-session turn token; the settled entry is removed
    /// only when it is still the caller's own turn (TS finally-check).
    preset_switches: Arc<
        parking_lot::Mutex<
            std::collections::HashMap<
                dsh_session::SessionId,
                (
                    u64,
                    futures::future::Shared<
                        BoxFuture<'static, Arc<RpcResponse<serde_json::Value>>>,
                    >,
                ),
            >,
        >,
    >,
    /// Monotone turn tokens for the preset-switch chains.
    preset_switch_counter: std::sync::atomic::AtomicU64,
    /// Pending approval/question requests and live mux subscribers.
    interactions: Arc<crate::interactions::InteractionState>,
}

impl cordis::Service for ApiProxyService {
    fn service_name(&self) -> &'static str {
        "apiProxy"
    }
}

impl ApiProxyService {
    /// Construct and register the `apiProxy` service (TS
    /// `createApiProxy`'s constructor half).
    pub fn install(ctx: &Context, defaults: ApiProxyDefaults) -> Arc<Self> {
        let defaults = Arc::new(defaults);
        let selection_defaults = defaults.clone();
        let agent_options: Arc<dyn Fn() -> dsh_agent::AgentOptions + Send + Sync> =
            Arc::new(move || {
                let selection = (selection_defaults.default_model_selection)();
                dsh_agent::AgentOptions {
                    provider: Some(selection.provider),
                    model: Some(selection.model),
                    ..Default::default()
                }
            });
        let resolver = crate::agent_lookup::AgentResolver::new(
            ctx,
            crate::agent_lookup::ApiRemoteAgentOptions {
                agent_options,
                setup: None,
            },
        );
        let interactions = crate::interactions::InteractionState::new();
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            defaults,
            resolver,
            selections: parking_lot::Mutex::new(std::collections::HashMap::new()),
            preset_switches: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            preset_switch_counter: std::sync::atomic::AtomicU64::new(0),
            interactions: interactions.clone(),
        });
        ctx.register_service(service.clone());
        interactions.activate(ctx);
        service
    }

    fn directory_picker(&self) -> Option<Arc<dyn DirectoryPicker>> {
        self.ctx
            .get_typed::<Arc<dyn DirectoryPicker>>("directoryPicker", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn agents(&self) -> Option<Arc<dsh_agent::AgentRegistry>> {
        self.ctx
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn sessions(&self) -> Option<Arc<dsh_session::SessionStore>> {
        self.ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn agent_presets(&self) -> Option<Arc<dsh_agent_presets::AgentPresets>> {
        self.ctx
            .get_typed::<Arc<dsh_agent_presets::AgentPresets>>("agentPresets", false)
            .map(|slot| slot.as_ref().clone())
    }

    /// Whether this deployment can hand a path to a native opener at all
    /// (TS `canOpenPaths`): an injected decision wins, then an injected
    /// opener, then the platform probe.
    fn can_open_paths(&self) -> bool {
        if let Some(can) = &self.defaults.can_open_path {
            return can();
        }
        self.defaults.open_path.is_some()
            || crate::native_path_opener::can_open_native_path(
                &crate::native_path_opener::PathOpenerInternals::default(),
            )
    }

    /// The refusal for a deployment that composes no preset roster
    /// (TS `noRoster`).
    fn no_roster(&self, rpc_id: RpcId, agent_preset: &str) -> RpcResponse<serde_json::Value> {
        err(
            rpc_id,
            RpcError::AgentPresetNotFound(RpcErrorBody {
                message: "this deployment composes no agent presets".to_string(),
                details: crate::api::rpc::AgentPresetNotFoundDetails {
                    agent_preset: agent_preset.to_string(),
                    available: Vec::new(),
                },
            }),
        )
    }

    /// Map one authoring/roster failure onto its wire code (TS
    /// `presetError`). The service's `read`/`copy`/`remove` surface errors
    /// as thiserror-rendered strings whose templates are fixed and whose
    /// preset ids are confined to `[a-z0-9-]`, so the classification below
    /// is exact.
    fn preset_error(
        &self,
        rpc_id: RpcId,
        agent_preset: &str,
        error: String,
    ) -> RpcResponse<serde_json::Value> {
        if let Some(rest) = error.strip_prefix("agent-presets: preset \"") {
            if let Some((id, tail)) = rest.split_once('"') {
                if let Some(available_tail) = tail.strip_prefix(" not found (available: ") {
                    let available_tail = available_tail.strip_suffix(')').unwrap_or(available_tail);
                    let available: Vec<String> = if available_tail == "none" {
                        Vec::new()
                    } else {
                        available_tail.split(", ").map(str::to_string).collect()
                    };
                    return err(
                        rpc_id,
                        RpcError::AgentPresetNotFound(RpcErrorBody {
                            message: error.clone(),
                            details: crate::api::rpc::AgentPresetNotFoundDetails {
                                agent_preset: id.to_string(),
                                available,
                            },
                        }),
                    );
                }
                if let Some(reason) = tail.strip_prefix(" failed to mount: ") {
                    return err(
                        rpc_id,
                        RpcError::AgentPresetInvalid(RpcErrorBody {
                            message: error.clone(),
                            details: crate::api::rpc::AgentPresetReasonDetails {
                                agent_preset: agent_preset.to_string(),
                                reason: reason.to_string(),
                            },
                        }),
                    );
                }
                if tail.starts_with(" cannot be written: ") {
                    return err(
                        rpc_id,
                        RpcError::AgentPresetReadOnly(RpcErrorBody {
                            message: error.clone(),
                            details: crate::api::rpc::AgentPresetReasonDetails {
                                agent_preset: agent_preset.to_string(),
                                reason: error,
                            },
                        }),
                    );
                }
                if tail.starts_with(" already exists") {
                    return err(
                        rpc_id,
                        RpcError::AgentPresetInvalid(RpcErrorBody {
                            message: error.clone(),
                            details: crate::api::rpc::AgentPresetReasonDetails {
                                agent_preset: agent_preset.to_string(),
                                reason: error,
                            },
                        }),
                    );
                }
            }
        }
        if error.starts_with("agent-presets: preset id ") {
            return err(
                rpc_id,
                RpcError::AgentPresetInvalid(RpcErrorBody {
                    message: error.clone(),
                    details: crate::api::rpc::AgentPresetReasonDetails {
                        agent_preset: agent_preset.to_string(),
                        reason: error,
                    },
                }),
            );
        }
        err(
            rpc_id,
            RpcError::Internal(RpcErrorBody {
                message: format!("agent preset \"{agent_preset}\": {error}"),
                details: EmptyDetails {},
            }),
        )
    }

    /// The refusal a typed preset failure becomes during session-create /
    /// select (TS `presetFailure`).
    fn preset_failure_unknown(
        &self,
        rpc_id: RpcId,
        error: dsh_agent_presets::UnknownPresetError,
    ) -> RpcResponse<serde_json::Value> {
        err(
            rpc_id,
            RpcError::AgentPresetNotFound(RpcErrorBody {
                message: error.to_string(),
                details: crate::api::rpc::AgentPresetNotFoundDetails {
                    agent_preset: error.preset_id,
                    available: if error.available == "none" {
                        Vec::new()
                    } else {
                        error.available.split(", ").map(str::to_string).collect()
                    },
                },
            }),
        )
    }

    async fn host_describe(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        let selection = (self.defaults.default_model_selection)();
        let attached_sessions = self
            .agents()
            .map(|registry| registry.list().len() as u64)
            .unwrap_or(0);
        let can_open_path = self
            .defaults
            .can_open_path
            .as_ref()
            .map(|probe| probe())
            .unwrap_or_else(|| self.defaults.open_path.is_some());
        ok(
            request.rpc_id,
            HostDescribeResult {
                version: HOST_VERSION.to_string(),
                cwd: self.defaults.cwd.clone(),
                provider: Some(selection.provider),
                model: Some(selection.model),
                attached_sessions,
                can_open_path,
            },
        )
    }

    async fn host_pick_directory(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let Some(picker) = self.directory_picker() else {
            return err(
                request.rpc_id,
                RpcError::DirectoryPickerUnavailable(RpcErrorBody {
                    message: "host.pickDirectory: no directoryPicker service is composed"
                        .to_string(),
                    details: crate::api::rpc::CapabilityDetails {
                        capability: "absent".to_string(),
                    },
                }),
            );
        };
        let DirectoryPickerCapability::Native(native) = picker.capability() else {
            let kind = picker.capability().kind();
            return err(
                request.rpc_id,
                RpcError::DirectoryPickerUnavailable(RpcErrorBody {
                    message: format!(
                        "host.pickDirectory needs the native capability; the composed picker serves \"{kind}\""
                    ),
                    details: crate::api::rpc::CapabilityDetails {
                        capability: kind.to_string(),
                    },
                }),
            );
        };
        // The picker signal is the caller's connection lifetime.
        let picker_signal = PickerAbort::new();
        let pick = (native.pick)(picker_signal);
        tokio::pin!(pick);
        let picked = tokio::select! {
            biased;
            _ = signal.cancelled() => None,
            picked = &mut pick => picked,
        };
        ok(request.rpc_id, HostPickDirectoryResult { path: picked })
    }

    async fn host_list_directory(
        &self,
        request: RpcRequest<HostListDirectoryRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let browse = self.browse_capability();
        let Some(browse) = browse else {
            return err(
                request.rpc_id,
                RpcError::DirectoryPickerUnavailable(RpcErrorBody {
                    message:
                        "host.listDirectory: no browse-capable directoryPicker service is composed"
                            .to_string(),
                    details: crate::api::rpc::CapabilityDetails {
                        capability: "absent".to_string(),
                    },
                }),
            );
        };
        let picker_signal = PickerAbort::new();
        let list = (browse.list)(request.payload.path, picker_signal);
        tokio::pin!(list);
        let listed = tokio::select! {
            biased;
            _ = signal.cancelled() => Err(DirectoryPickerListError::Aborted),
            listed = &mut list => listed,
        };
        match listed {
            Ok(listing) => ok(request.rpc_id, listing),
            Err(DirectoryPickerListError::Aborted) => err(
                request.rpc_id,
                RpcError::Cancelled(RpcErrorBody {
                    message: "host.listDirectory: caller left".to_string(),
                    details: EmptyDetails {},
                }),
            ),
            Err(DirectoryPickerListError::Unreadable(error)) => {
                let code = match error.code {
                    DirectoryPickerErrorCode::DirectoryUnreadable => {
                        crate::api::rpc::RpcErrorCode::DirectoryUnreadable
                    }
                    other => {
                        let _ = other;
                        crate::api::rpc::RpcErrorCode::Internal
                    }
                };
                err(
                    request.rpc_id,
                    code_rpc_error(code, &error.path, &error.message),
                )
            }
        }
    }

    fn browse_capability(&self) -> Option<DirectoryPickerBrowseCapability> {
        let picker = self.directory_picker()?;
        match picker.capability() {
            DirectoryPickerCapability::Browse(browse) => Some(browse),
            _ => None,
        }
    }

    async fn host_create_directory(
        &self,
        request: RpcRequest<HostCreateDirectoryRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(browse) = self.browse_capability() else {
            return err(
                request.rpc_id,
                RpcError::DirectoryPickerUnavailable(RpcErrorBody {
                    message: "host.createDirectory: no browse-capable directoryPicker service is composed".to_string(),
                    details: crate::api::rpc::CapabilityDetails {
                        capability: "absent".to_string(),
                    },
                }),
            );
        };
        match (browse.create_directory)(request.payload.path, request.payload.name).await {
            Ok(path) => ok(request.rpc_id, HostCreateDirectoryResult { path }),
            Err(error) => {
                let code = match error.code {
                    DirectoryPickerErrorCode::DirectoryExists => {
                        crate::api::rpc::RpcErrorCode::DirectoryExists
                    }
                    DirectoryPickerErrorCode::DirectoryCreateFailed => {
                        crate::api::rpc::RpcErrorCode::DirectoryCreateFailed
                    }
                    DirectoryPickerErrorCode::DirectoryUnreadable => {
                        crate::api::rpc::RpcErrorCode::Internal
                    }
                };
                err(
                    request.rpc_id,
                    code_rpc_error(code, &error.path, &error.message),
                )
            }
        }
    }

    async fn host_open_path(
        &self,
        request: RpcRequest<HostOpenPathRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let Some(open_path) = &self.defaults.open_path else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "host.openPath: no native opener is composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        match open_path(request.payload.path, signal).await {
            Ok(()) => ok(request.rpc_id, HostOpenPathResult { opened: true }),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("host.openPath: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
    async fn skill_list(
        &self,
        request: RpcRequest<crate::api::skills::SkillListRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::skills::SkillEntry;

        let sessions = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone());
        let Some(sessions) = sessions else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "skill.list: the sessions service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(session) = sessions.get(&request.payload.session_id) else {
            return err(
                request.rpc_id,
                RpcError::SessionNotFound(RpcErrorBody {
                    message: format!(
                        "session \"{}\" not found (not attached)",
                        request.payload.session_id
                    ),
                    details: crate::api::rpc::SessionIdDetails {
                        session_id: request.payload.session_id.to_string(),
                    },
                }),
            );
        };
        let Some(cwd) = &session.header().cwd else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!(
                        "session \"{}\" has no project cwd",
                        request.payload.session_id
                    ),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(registry) = self
            .ctx
            .get_typed::<Arc<dsh_skill::SkillRegistry>>("skills", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "skill registry is absent: neither this session's agent preset nor the host composition mounts dsh-skill".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        // The scope presenters resolve in — the live agent, else the
        // recorded preset's standing key, else the global layer; the Rust
        // composition reads the global layer until the preset milestone.
        let options = dsh_skill::SkillViewOptions {
            cwd: Some(cwd.clone()),
            signal: None,
            scope: None,
        };
        match registry.list(options).await {
            Ok(skills) => {
                let entries: Vec<SkillEntry> = skills
                    .into_iter()
                    .filter(dsh_skill::is_user_invocable)
                    .map(|skill| SkillEntry {
                        name: skill.name,
                        description: skill.description,
                        when_to_use: skill.when_to_use,
                        model_invocable: skill.invocation.model_invocable,
                    })
                    .collect();
                ok(
                    request.rpc_id,
                    crate::api::skills::SkillListResult { skills: entries },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("skill listing failed: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
    fn credentials(&self) -> Option<Arc<dyn dsh_credentials::CredentialProvider>> {
        self.ctx
            .get_typed::<Arc<dyn dsh_credentials::CredentialProvider>>("credentials", false)
            .map(|slot| slot.as_ref().clone())
    }

    /// The seam's reference-shape rule (TS `REF_PATTERN`), checked without a
    /// regex dependency: a POSIX shell identifier.
    fn valid_ref(value: &str) -> bool {
        let mut chars = value.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
            _ => return false,
        }
        chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }

    async fn credentials_describe(
        &self,
        request: RpcRequest<crate::api::credentials::CredentialsDescribeRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::credentials::CredentialView;
        use dsh_credentials::CredentialRef;

        let Some(provider) = self.credentials() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "credentials.describe: no credentials service is composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        for reference in &request.payload.references {
            if !Self::valid_ref(reference) {
                return err(
                    request.rpc_id,
                    RpcError::BadRequest(RpcErrorBody {
                        message: format!("invalid credential reference \"{reference}\""),
                        details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                    }),
                );
            }
        }
        let mut credentials = indexmap::IndexMap::new();
        for reference in &request.payload.references {
            let info = provider
                .describe(&CredentialRef::new(reference.clone()))
                .await;
            credentials.insert(
                reference.clone(),
                CredentialView {
                    configured: info.configured,
                    source: info.source,
                    writable: info.writable,
                },
            );
        }
        ok(
            request.rpc_id,
            crate::api::credentials::CredentialsDescribeResult { credentials },
        )
    }

    async fn credentials_set(
        &self,
        request: RpcRequest<crate::api::credentials::CredentialsSetRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use dsh_credentials::CredentialRef;

        let Some(provider) = self.credentials() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "credentials.set: no credentials service is composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        if !Self::valid_ref(&request.payload.reference) {
            return err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message: format!(
                        "invalid credential reference \"{}\"",
                        request.payload.reference
                    ),
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            );
        }
        match provider
            .set(
                &CredentialRef::new(request.payload.reference.clone()),
                &request.payload.value,
            )
            .await
        {
            Ok(()) => ok(request.rpc_id, serde_json::json!({})),
            Err(error) => err(
                request.rpc_id,
                RpcError::CredentialRejected(RpcErrorBody {
                    message: error,
                    details: crate::api::rpc::CredentialRefDetails {
                        reference: request.payload.reference,
                    },
                }),
            ),
        }
    }

    async fn credentials_unset(
        &self,
        request: RpcRequest<crate::api::credentials::CredentialsUnsetRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use dsh_credentials::CredentialRef;

        let Some(provider) = self.credentials() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "credentials.unset: no credentials service is composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        if !Self::valid_ref(&request.payload.reference) {
            return err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message: format!(
                        "invalid credential reference \"{}\"",
                        request.payload.reference
                    ),
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            );
        }
        match provider
            .unset(&CredentialRef::new(request.payload.reference.clone()))
            .await
        {
            Ok(()) => ok(request.rpc_id, serde_json::json!({})),
            Err(error) => err(
                request.rpc_id,
                RpcError::CredentialRejected(RpcErrorBody {
                    message: error,
                    details: crate::api::rpc::CredentialRefDetails {
                        reference: request.payload.reference,
                    },
                }),
            ),
        }
    }
    /// The goal service visible to one exact live agent (preset-scoped
    /// lookup arrives with the preset milestone; the global layer for now).
    fn goal_service_for(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> Result<Arc<dsh_goal::GoalService>, RpcError> {
        agent
            .ctx()
            .get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                RpcError::Internal(RpcErrorBody {
                    message: "goal service is absent: neither this session's agent preset nor the host composition mounts dsh-goal".to_string(),
                    details: EmptyDetails {},
                })
            })
    }

    /// Map one goal-domain rejection to the wire error (TS `goalError`:
    /// internal, the stable GoalError code dropped from the empty details
    /// slot exactly like the TS schema strips it).
    fn goal_error<T>(rpc_id: RpcId, error: dsh_goal::GoalError) -> RpcResponse<T> {
        err(
            rpc_id,
            RpcError::Internal(RpcErrorBody {
                message: error.message,
                details: EmptyDetails {},
            }),
        )
    }

    fn wire_goal_ref(view: &dsh_goal::GoalView) -> crate::api::goals::GoalRef {
        crate::api::goals::GoalRef {
            id: crate::api::goals::GoalId::new(view.id.to_string()),
            revision: view.revision as i64,
        }
    }

    /// Resolve a session's agent, apply one goal mutation, and acknowledge
    /// with the new CAS ref (TS `mutateGoal`).
    async fn mutate_goal(
        &self,
        rpc_id: RpcId,
        session_id: &dsh_session::SessionId,
        mutation: Arc<
            dyn Fn(
                    Arc<dsh_goal::GoalService>,
                    Arc<dyn Agent>,
                ) -> Result<dsh_goal::GoalView, dsh_goal::GoalError>
                + Send
                + Sync,
        >,
    ) -> RpcResponse<serde_json::Value> {
        let resolved = self.resolver.resolve(session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(rpc_id, error);
            }
        };
        let goals = match self.goal_service_for(&agent) {
            Ok(goals) => goals,
            Err(error) => return err(rpc_id, error),
        };
        match mutation(goals, agent) {
            Ok(view) => {
                let goal_ref = Self::wire_goal_ref(&view);
                ok(rpc_id, crate::api::goals::GoalRefResult { goal_ref })
            }
            Err(error) => Self::goal_error(rpc_id, error),
        }
    }

    async fn goal_create(
        &self,
        request: RpcRequest<crate::api::goals::GoalCreateRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let session_id = request.payload.session_id.clone();
        let objective = request.payload.objective.clone();
        let max_goal_rounds = request.payload.max_goal_rounds;
        self.mutate_goal(
            rpc_id,
            &session_id,
            Arc::new(move |goals, agent| {
                goals.create(
                    &agent,
                    dsh_goal::CreateGoalRequest {
                        objective: objective.clone(),
                        max_goal_rounds,
                    },
                )
            }),
        )
        .await
    }

    async fn goal_edit(
        &self,
        request: RpcRequest<crate::api::goals::GoalEditRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let session_id = request.payload.session_id.clone();
        let goal_ref = request.payload.goal_ref;
        let objective = request.payload.objective.clone();
        let max_goal_rounds = request.payload.max_goal_rounds;
        self.mutate_goal(
            rpc_id,
            &session_id,
            Arc::new(move |goals, agent| {
                goals.edit(
                    &agent,
                    &dsh_goal::GoalRef {
                        id: dsh_goal::goal_id(goal_ref.id.to_string()),
                        revision: goal_ref.revision.max(0) as u64,
                    },
                    &dsh_goal::EditGoalRequest {
                        objective: objective.clone(),
                        max_goal_rounds,
                    },
                )
            }),
        )
        .await
    }

    fn goal_verb_ref(goal_ref: &crate::api::goals::GoalRef) -> dsh_goal::GoalRef {
        dsh_goal::GoalRef {
            id: dsh_goal::goal_id(goal_ref.id.to_string()),
            revision: goal_ref.revision.max(0) as u64,
        }
    }

    async fn goal_verb(
        &self,
        request: RpcRequest<crate::api::goals::GoalVerbRequest>,
        verb: GoalVerb,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let session_id = request.payload.session_id.clone();
        let goal_ref = request.payload.goal_ref.clone();
        self.mutate_goal(
            rpc_id,
            &session_id,
            Arc::new(move |goals, agent| {
                let goal_ref = Self::goal_verb_ref(&goal_ref);
                match verb {
                    GoalVerb::Pause => goals.pause(&agent, &goal_ref),
                    GoalVerb::Resume => goals.resume(&agent, &goal_ref),
                    GoalVerb::Complete => goals.complete(&agent, &goal_ref),
                    GoalVerb::Clear => unreachable!("clear answers a plain acknowledgement"),
                }
            }),
        )
        .await
    }

    async fn goal_clear(
        &self,
        request: RpcRequest<crate::api::goals::GoalClearRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let session_id = request.payload.session_id.clone();
        let goal_ref = request.payload.goal_ref.clone();
        let resolved = self.resolver.resolve(&session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(rpc_id, error);
            }
        };
        let goals = match self.goal_service_for(&agent) {
            Ok(goals) => goals,
            Err(error) => return err(rpc_id, error),
        };
        match goals.clear(&agent, &Self::goal_verb_ref(&goal_ref)) {
            Ok(_) => ok(rpc_id, crate::api::goals::GoalClearResult { cleared: true }),
            Err(error) => Self::goal_error(rpc_id, error),
        }
    }
}

/// The ref-carrying goal verbs.
#[derive(Clone, Copy)]
enum GoalVerb {
    Pause,
    Resume,
    Complete,
    Clear,
}

impl ApiProxyService {
    fn llm_runtime(&self) -> Option<Arc<dsh_llm::LlmRuntime>> {
        self.ctx
            .get_typed::<Arc<dsh_llm::LlmRuntime>>("llm", false)
            .map(|slot| slot.as_ref().clone())
    }

    async fn llm_providers(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::llm::ConfigurableProviderView;

        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "llm.providers: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let registered = runtime.list_providers();
        let active: std::collections::HashSet<String> = registered
            .iter()
            .map(|provider| provider.id.clone())
            .collect();
        let directory = runtime.list_configurable_providers();
        let declared: std::collections::HashSet<String> = directory
            .iter()
            .map(|entry| entry.provider.clone())
            .collect();
        let mut views: Vec<ConfigurableProviderView> = directory
            .into_iter()
            .map(|entry| ConfigurableProviderView {
                provider: entry.provider.clone(),
                display_name: entry.display_name,
                settings_ns: entry.settings_ns,
                settings_path: entry.settings_path,
                active: active.contains(&entry.provider),
                declared: entry.declared,
            })
            .collect();
        // Routes registered without a directory declaration still appear —
        // they exist and serve models — just with no settings address.
        for provider in registered {
            if declared.contains(&provider.id) {
                continue;
            }
            views.push(ConfigurableProviderView {
                provider: provider.id,
                display_name: provider.name,
                settings_ns: String::new(),
                settings_path: Vec::new(),
                active: true,
                declared: None,
            });
        }
        ok(
            request.rpc_id,
            crate::api::llm::LlmProvidersResult { providers: views },
        )
    }

    /// Build the host-scoped model catalog (TS `buildModelCatalog`).
    async fn build_model_catalog(
        runtime: &Arc<dsh_llm::LlmRuntime>,
    ) -> crate::api::llm::LlmModelsResult {
        use crate::api::sessions::{
            ModelCatalogFailure, ModelCatalogModel, ModelProviderGroup, ModelReasoning,
            ModelReasoningEffort,
        };

        let mut groups: Vec<ModelProviderGroup> = Vec::new();
        let mut failures: Vec<ModelCatalogFailure> = Vec::new();
        for provider in runtime.list_providers() {
            match runtime.list_models(&provider.id).await {
                Ok(models) => {
                    let mut entries: Vec<ModelCatalogModel> = Vec::new();
                    for model in models {
                        let resolved = runtime
                            .resolve_model_info(&provider.id, &model.id, None)
                            .await
                            .map_err(|error| error.to_string());
                        match resolved {
                            Ok(resolved) => {
                                let reasoning =
                                    resolved.reasoning.map(|reasoning| ModelReasoning {
                                        efforts: reasoning
                                            .efforts
                                            .into_iter()
                                            .map(|effort| ModelReasoningEffort {
                                                id: effort.id.to_string(),
                                                name: effort.name,
                                                description: effort.description,
                                            })
                                            .collect(),
                                        default_effort: reasoning
                                            .default_effort
                                            .map(|id| id.to_string()),
                                    });
                                entries.push(ModelCatalogModel {
                                    id: model.id,
                                    name: model.name,
                                    description: model.description,
                                    reasoning,
                                });
                            }
                            Err(error) => {
                                failures.push(ModelCatalogFailure {
                                    id: provider.id.clone(),
                                    name: provider.name.clone(),
                                    message: error,
                                });
                            }
                        }
                    }
                    groups.push(ModelProviderGroup {
                        id: provider.id,
                        name: provider.name,
                        models: entries,
                    });
                }
                Err(error) => {
                    failures.push(ModelCatalogFailure {
                        id: provider.id,
                        name: provider.name,
                        message: error.to_string(),
                    });
                }
            }
        }
        // The TS catalog filters empty groups (a provider whose listing
        // succeeded but resolved nothing contributes neither group nor
        // failure).
        groups.retain(|group| !group.models.is_empty());
        crate::api::llm::LlmModelsResult { groups, failures }
    }

    async fn llm_models(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "llm.models: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        ok(request.rpc_id, Self::build_model_catalog(&runtime).await)
    }

    async fn llm_discover_models(
        &self,
        request: RpcRequest<crate::api::llm::LlmDiscoverModelsRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::llm::DiscoveredModelView;

        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "llm.discoverModels: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let payload = &request.payload;
        let abort_flag = Arc::new(move || signal.aborted());
        match runtime
            .discover_models(
                &payload.settings_ns,
                &dsh_llm::LlmModelDiscoveryRequest {
                    provider: payload.provider.clone(),
                    base_url: payload.base_url.clone(),
                    api: payload.api.clone(),
                    api_key: payload.api_key.clone(),
                    signal: Some(abort_flag),
                },
            )
            .await
        {
            Ok(models) => {
                let views: Vec<DiscoveredModelView> = models
                    .into_iter()
                    .map(|model| DiscoveredModelView {
                        id: model.id,
                        name: model.name,
                        context_window: model.context_window,
                        max_tokens: model.max_tokens,
                    })
                    .collect();
                ok(
                    request.rpc_id,
                    crate::api::llm::LlmDiscoverModelsResult { models: views },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::ModelDiscoveryFailed(RpcErrorBody {
                    message: error.to_string(),
                    details: crate::api::rpc::ModelDiscoveryFailedDetails {
                        settings_ns: payload.settings_ns.clone(),
                        base_url: payload.base_url.clone(),
                    },
                }),
            ),
        }
    }
}

impl ApiProxyService {
    fn settings_provider(&self) -> Option<Arc<dsh_settings::SettingsProvider>> {
        self.ctx
            .get_typed::<Arc<dsh_settings::SettingsProvider>>("settings", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn settings_absent() -> RpcError {
        RpcError::Internal(RpcErrorBody {
            message: "settings service is absent: the host composition does not mount dsh-settings"
                .to_string(),
            details: EmptyDetails {},
        })
    }

    async fn settings_describe(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::settings::{SettingsNamespaceView, SettingsSecretView};
        use dsh_settings::SettingsApplies;

        let Some(provider) = self.settings_provider() else {
            return err(request.rpc_id, Self::settings_absent());
        };
        let writable = provider.writable();
        let has_document = provider.document_path().is_some();
        let namespaces: Vec<SettingsNamespaceView> = provider
            .describe(dsh_settings::SettingsDescribeOptions {
                redact_secrets: true,
            })
            .into_iter()
            .map(|descriptor| {
                let applies = match descriptor.applies {
                    SettingsApplies::Live => crate::api::settings::SettingsApplies::Live,
                    SettingsApplies::Restart => crate::api::settings::SettingsApplies::Restart,
                };
                SettingsNamespaceView {
                    ns: descriptor.ns.to_string(),
                    schema: descriptor.schema,
                    value: descriptor
                        .value
                        .to_json()
                        .unwrap_or(serde_json::Value::Null),
                    base: descriptor
                        .base
                        .map(|base| base.to_json().unwrap_or(serde_json::Value::Null)),
                    user: descriptor
                        .user
                        .map(|user| user.to_json().unwrap_or(serde_json::Value::Null)),
                    applies,
                    secrets: descriptor
                        .secrets
                        .into_iter()
                        .map(|secret| SettingsSecretView {
                            path: secret.path,
                            set: secret.set,
                        })
                        .collect(),
                    revision: descriptor.revision as i64,
                }
            })
            .collect();
        ok(
            request.rpc_id,
            crate::api::settings::SettingsDescribeResult {
                writable,
                has_document,
                namespaces,
            },
        )
    }

    async fn settings_write(
        &self,
        rpc_id: RpcId,
        ns: String,
        operation: SettingsWrite,
    ) -> RpcResponse<serde_json::Value> {
        let Some(provider) = self.settings_provider() else {
            return err(rpc_id, Self::settings_absent());
        };
        let namespace = dsh_settings::SettingsNamespace::new(ns.clone());
        let outcome = match operation {
            SettingsWrite::Update {
                patch,
                expected_revision,
            } => provider.update(&namespace, patch, expected_revision).await,
            SettingsWrite::Replace {
                section,
                expected_revision,
            } => {
                provider
                    .replace(&namespace, section, expected_revision)
                    .await
            }
            SettingsWrite::Mutate {
                ops,
                expected_revision,
            } => {
                let ops: Vec<dsh_settings::SettingsPathOp> = ops
                    .into_iter()
                    .map(|op| match op {
                        crate::api::settings::SettingsPathOpView::Set { path, value } => {
                            dsh_settings::SettingsPathOp::Set { path, value }
                        }
                        crate::api::settings::SettingsPathOpView::Unset { path } => {
                            dsh_settings::SettingsPathOp::Unset { path }
                        }
                    })
                    .collect();
                provider.mutate(&namespace, ops, expected_revision).await
            }
        };
        match outcome {
            Ok(()) => {
                // Answer with the namespace's new redacted view.
                let descriptor = provider
                    .describe(dsh_settings::SettingsDescribeOptions {
                        redact_secrets: true,
                    })
                    .into_iter()
                    .find(|descriptor| descriptor.ns.as_str() == ns);
                let Some(descriptor) = descriptor else {
                    return err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!(
                                "settings namespace \"{ns}\" disappeared after the write"
                            ),
                            details: EmptyDetails {},
                        }),
                    );
                };
                ok(
                    rpc_id,
                    serde_json::to_value(crate::api::settings::SettingsNamespaceView {
                        ns: descriptor.ns.to_string(),
                        schema: descriptor.schema,
                        value: descriptor
                            .value
                            .to_json()
                            .unwrap_or(serde_json::Value::Null),
                        base: descriptor
                            .base
                            .map(|base| base.to_json().unwrap_or(serde_json::Value::Null)),
                        user: descriptor
                            .user
                            .map(|user| user.to_json().unwrap_or(serde_json::Value::Null)),
                        applies: match descriptor.applies {
                            dsh_settings::SettingsApplies::Live => {
                                crate::api::settings::SettingsApplies::Live
                            }
                            dsh_settings::SettingsApplies::Restart => {
                                crate::api::settings::SettingsApplies::Restart
                            }
                        },
                        secrets: descriptor
                            .secrets
                            .into_iter()
                            .map(|secret| crate::api::settings::SettingsSecretView {
                                path: secret.path,
                                set: secret.set,
                            })
                            .collect(),
                        revision: descriptor.revision as i64,
                    })
                    .expect("namespace views serialize"),
                )
            }
            Err(error) => {
                if error.contains("changed since it was read") {
                    let (expected, actual) = parse_conflict_revisions(&error);
                    return err(
                        rpc_id,
                        RpcError::SettingsConflict(RpcErrorBody {
                            message: error,
                            details: crate::api::rpc::SettingsConflictDetails {
                                ns: ns.clone(),
                                expected,
                                actual,
                            },
                        }),
                    );
                }
                err(
                    rpc_id,
                    RpcError::SettingsRejected(RpcErrorBody {
                        message: error,
                        details: crate::api::rpc::NamespaceDetails { ns },
                    }),
                )
            }
        }
    }

    async fn settings_open_document(
        &self,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let Some(provider) = self.settings_provider() else {
            return err(request.rpc_id, Self::settings_absent());
        };
        if signal.aborted() {
            return err(
                request.rpc_id,
                RpcError::Cancelled(RpcErrorBody {
                    message: "settings document open was aborted".to_string(),
                    details: EmptyDetails {},
                }),
            );
        }
        let Some(path) = provider.prepare_document().await else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "settings provider has no local document to open".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(open_path) = &self.defaults.open_path else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "settings.openDocument: no native opener is composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        match open_path(path, signal).await {
            Ok(()) => ok(
                request.rpc_id,
                crate::api::settings::SettingsOpenDocumentResult { opened: true },
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("settings.openDocument: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
}

/// Parse the expected/actual revision pair from the provider's conflict
/// message (stable prefix `(expected revision {expected}, now {actual})`).
fn parse_conflict_revisions(message: &str) -> (i64, i64) {
    let expected = message
        .find("(expected revision ")
        .and_then(|start| {
            let rest = &message[start + "(expected revision ".len()..];
            rest.split(',')
                .next()
                .and_then(|part| part.trim().parse::<i64>().ok())
        })
        .unwrap_or(0);
    let actual = message
        .find(", now ")
        .and_then(|start| {
            let rest = &message[start + ", now ".len()..];
            rest.split(')')
                .next()
                .and_then(|part| part.trim().parse::<i64>().ok())
        })
        .unwrap_or(0);
    (expected, actual)
}

/// The settings write verbs.
enum SettingsWrite {
    Update {
        patch: serde_json::Value,
        expected_revision: Option<u64>,
    },
    Replace {
        section: serde_json::Value,
        expected_revision: Option<u64>,
    },
    Mutate {
        ops: Vec<crate::api::settings::SettingsPathOpView>,
        expected_revision: Option<u64>,
    },
}

impl ApiProxyService {
    fn workspace_registry(&self) -> Option<Arc<dsh_workspace::WorkspaceRegistry>> {
        self.ctx
            .get_typed::<Arc<dsh_workspace::WorkspaceRegistry>>("workspaceRegistry", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn workspace_absent() -> RpcError {
        RpcError::Internal(RpcErrorBody {
            message:
                "workspace registry is absent: the host composition does not mount dsh-workspace"
                    .to_string(),
            details: EmptyDetails {},
        })
    }

    /// Project one domain workspace into its wire view.
    fn workspace_view(
        workspace: &dsh_workspace::Workspace,
    ) -> crate::api::workspace::WorkspaceView {
        crate::api::workspace::WorkspaceView {
            workspace_id: crate::api::workspace::WorkspaceId::new(workspace.id().to_string()),
            path: workspace.path(),
            title: workspace.title(),
            session_ids: workspace
                .session_ids()
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            created_at: workspace.created_at(),
            updated_at: workspace.updated_at(),
        }
    }

    /// Project one workspaces-table record into its wire view (TS
    /// `changedWorkspaceView`).
    fn workspace_record_view(
        key: &str,
        record: &serde_json::Value,
    ) -> Option<crate::api::workspace::WorkspaceView> {
        Some(crate::api::workspace::WorkspaceView {
            workspace_id: crate::api::workspace::WorkspaceId::new(key.to_string()),
            path: record.get("path")?.as_str()?.to_string(),
            title: record.get("title")?.as_str()?.to_string(),
            session_ids: record
                .get("sessionIds")?
                .as_array()?
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect(),
            created_at: record.get("createdAt")?.as_str()?.to_string(),
            updated_at: record.get("updatedAt")?.as_str()?.to_string(),
        })
    }

    async fn workspace_list(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        match registry.list() {
            Ok(workspaces) => {
                let items: Vec<crate::api::workspace::WorkspaceView> =
                    workspaces.iter().map(Self::workspace_view).collect();
                let archived_session_ids: Vec<String> = registry
                    .archived_session_ids()
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect();
                ok(
                    request.rpc_id,
                    crate::api::workspace::WorkspaceListResult {
                        items,
                        archived_session_ids,
                    },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace.list: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    async fn workspace_create(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceCreateRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::workspace::{WorkspaceCreateResult, WorkspaceView};

        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let path = request.payload.path.clone();
        // The `created` bit: the registry reuses an existing path, and the
        // Rust `create` collapses that answer — a path match on the current
        // list (verbatim-prefix-stripped) approximates it (deviation: the
        // TS registry reports the created bit itself).
        let existed = registry
            .list()
            .ok()
            .map(|workspaces| {
                workspaces.iter().any(|workspace| {
                    workspace
                        .path()
                        .strip_prefix(r"\\?\")
                        .unwrap_or(workspace.path().as_str())
                        == path
                })
            })
            .unwrap_or(false);
        match registry.create(&path, None).await {
            Ok(workspace) => ok(
                request.rpc_id,
                WorkspaceCreateResult {
                    workspace: WorkspaceView::clone(&Self::workspace_view(&workspace)),
                    created: !existed,
                },
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::WorkspaceInvalidPath(RpcErrorBody {
                    message: error,
                    details: crate::api::rpc::PathDetails { path },
                }),
            ),
        }
    }

    async fn workspace_rename(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceRenameRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let workspace_id = dsh_workspace::workspace_id(request.payload.workspace_id.to_string());
        let Some(workspace) = registry.get(&workspace_id) else {
            return err(
                request.rpc_id,
                RpcError::WorkspaceNotFound(RpcErrorBody {
                    message: format!("workspace \"{workspace_id}\" not found"),
                    details: crate::api::rpc::WorkspaceIdDetails {
                        workspace_id: workspace_id.to_string(),
                    },
                }),
            );
        };
        let title = request.payload.title.trim().to_string();
        if title == workspace.title() {
            return ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceRenameResult {
                    workspace: Self::workspace_view(&workspace),
                },
            );
        }
        let conflicts = registry
            .list()
            .ok()
            .map(|workspaces| {
                workspaces
                    .iter()
                    .any(|other| other.id() != workspace.id() && other.title() == title)
            })
            .unwrap_or(false);
        if conflicts {
            return err(
                request.rpc_id,
                RpcError::WorkspaceNameConflict(RpcErrorBody {
                    message: format!("a workspace named \"{title}\" already exists"),
                    details: crate::api::rpc::NameDetails { name: title },
                }),
            );
        }
        match workspace.set_title(&title).await {
            Ok(()) => ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceRenameResult {
                    workspace: Self::workspace_view(&workspace),
                },
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace.rename: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    async fn workspace_delete(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceDeleteRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let workspace_id = dsh_workspace::workspace_id(request.payload.workspace_id.to_string());
        match dsh_workspace::WorkspaceRegistry::delete(&registry, &workspace_id).await {
            Ok(true) => ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceDeleteResult { deleted: true },
            ),
            Ok(false) => err(
                request.rpc_id,
                RpcError::WorkspaceNotFound(RpcErrorBody {
                    message: format!("workspace \"{workspace_id}\" not found"),
                    details: crate::api::rpc::WorkspaceIdDetails {
                        workspace_id: workspace_id.to_string(),
                    },
                }),
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace.delete: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    async fn workspace_insert_before(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceInsertBeforeRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let workspace_id = dsh_workspace::workspace_id(request.payload.workspace_id.to_string());
        let before = request
            .payload
            .before_workspace_id
            .as_ref()
            .map(|id| dsh_workspace::workspace_id(id.to_string()));
        match dsh_workspace::WorkspaceRegistry::insert_before(
            &registry,
            &workspace_id,
            before.as_ref(),
        )
        .await
        {
            Ok(ids) => ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceInsertBeforeResult {
                    workspace_ids: ids.into_iter().map(|id| id.to_string()).collect(),
                },
            ),
            Err(error) if error.contains("cannot reorder unknown workspace") => err(
                request.rpc_id,
                RpcError::WorkspaceNotFound(RpcErrorBody {
                    message: error,
                    details: crate::api::rpc::WorkspaceIdDetails {
                        workspace_id: workspace_id.to_string(),
                    },
                }),
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace.insertBefore: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    async fn workspace_archive_session(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceArchiveSessionRequest>,
        unarchive: bool,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let session_id = dsh_session::session_id(request.payload.session_id.clone());
        let outcome = if unarchive {
            dsh_workspace::WorkspaceRegistry::unarchive_session(&registry, &session_id).await
        } else {
            dsh_workspace::WorkspaceRegistry::archive_session(&registry, &session_id).await
        };
        match outcome {
            Ok(()) => ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceArchiveSessionResult {
                    archived_session_ids: registry
                        .archived_session_ids()
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect(),
                },
            ),
            Err(error) if error.contains("cannot archive session") => err(
                request.rpc_id,
                RpcError::SessionNotFound(RpcErrorBody {
                    message: error,
                    details: crate::api::rpc::SessionIdDetails {
                        session_id: session_id.to_string(),
                    },
                }),
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace archive: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
}

impl ApiProxyService {
    /// Summarize one attached session (TS `summarize` core fields; the
    /// projection block arrives with the projection milestone).
    fn summarize_attached(
        &self,
        session: &dsh_session::Session,
    ) -> crate::api::sessions::SessionSummary {
        let running = self
            .agents()
            .and_then(|registry| registry.get(session.id()))
            .is_some_and(|agent| agent.status() == dsh_agent::AgentStatus::Running);
        let header = session.header();
        let events = session.events();
        let blank = !events.iter().any(|event| event.type_ == "turn/start");
        let updated_at = events
            .iter()
            .rev()
            .find(|event| event.type_ == "user/message")
            .map(|event| event.time)
            .unwrap_or_else(|| header.created_at as i64);
        crate::api::sessions::SessionSummary {
            session_id: session.id().clone(),
            updated_at,
            running,
            blank,
            parent_session_id: header.parent_session.clone(),
            origin: header.origin.as_deref().and_then(|origin| match origin {
                "subagent" => Some(crate::api::sessions::SessionOrigin::Subagent),
                _ => None,
            }),
            cwd: header.cwd.clone(),
            agent_preset: header.agent_preset.clone(),
            projections: None,
        }
    }

    /// Summarize one cold session (TS `summarizeCold`; the cold blank probe
    /// is simplified — an unreadable artifact conservatively reports
    /// `blank: false`, the TS posture for oversized artifacts).
    fn summarize_cold(meta: &dsh_session::SessionHeader) -> crate::api::sessions::SessionSummary {
        crate::api::sessions::SessionSummary {
            session_id: meta.id.clone(),
            updated_at: meta.created_at as i64,
            running: false,
            blank: false,
            parent_session_id: meta.parent_session.clone(),
            origin: meta.origin.as_deref().and_then(|origin| match origin {
                "subagent" => Some(crate::api::sessions::SessionOrigin::Subagent),
                _ => None,
            }),
            cwd: meta.cwd.clone(),
            agent_preset: meta.agent_preset.clone(),
            projections: None,
        }
    }

    async fn session_list(
        &self,
        request: RpcRequest<crate::api::sessions::SessionListRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::SessionSummary;

        let Some(sessions) = self.sessions() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.list: the sessions service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let mut items: Vec<SessionSummary> = sessions
            .list()
            .iter()
            .map(|session| self.summarize_attached(session))
            .collect();
        let attached: std::collections::HashSet<String> = items
            .iter()
            .map(|item| item.session_id.to_string())
            .collect();
        if let Some(persistence) = self
            .ctx
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
            .map(|slot| slot.as_ref().clone())
        {
            if let Ok(cold) = persistence.list().await {
                for meta in cold {
                    if attached.contains(meta.id.as_str()) || meta.cwd.is_none() {
                        continue;
                    }
                    items.push(Self::summarize_cold(&meta));
                }
            }
        }
        // updatedAt descending (the TS sort).
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        ok(
            request.rpc_id,
            crate::api::sessions::SessionListResult { items },
        )
    }

    async fn session_create(
        &self,
        request: RpcRequest<crate::api::sessions::SessionCreateRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use dsh_session::CreateSessionMeta;

        let Some(sessions) = self.sessions() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.create: the sessions service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(agents) = self.agents() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.create: the agents service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        // Workspace attachment (ensureWorkspace) arrives with the
        // workspace-create milestone; a workspaceId request answers
        // internal for now.
        if request.payload.workspace_id.is_some() {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.create: workspace attachment is not implemented in the Rust composition yet".to_string(),
                    details: EmptyDetails {},
                }),
            );
        }
        let cwd = request
            .payload
            .cwd
            .clone()
            .unwrap_or_else(|| self.defaults.cwd.clone());
        let session_id = request.payload.session_id.clone();
        let meta = CreateSessionMeta {
            cwd: Some(cwd),
            agent_preset: request.payload.agent_preset.clone(),
            ..Default::default()
        };
        let session = match sessions
            .create(
                &self.ctx,
                session_id.clone(),
                Some(dsh_session::CreateSessionOptions {
                    meta: Some(meta),
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(session) => session,
            Err(error) => {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!("session.create: {error}"),
                        details: EmptyDetails {},
                    }),
                );
            }
        };
        // The idle agent rides the same creation (the factory is composed
        // by the host app; an absent factory is a composition failure).
        let agent_options = {
            let selection = (self.defaults.default_model_selection)();
            dsh_agent::AgentOptions {
                provider: Some(selection.provider),
                model: Some(selection.model),
                ..Default::default()
            }
        };
        match agents
            .create(dsh_agent::CreateAgentOptions {
                session_id: Some(session.id().clone()),
                agent_options: Some(agent_options),
                ..Default::default()
            })
            .await
        {
            Ok(handle) => {
                let _ = handle;
                ok(
                    request.rpc_id,
                    crate::api::sessions::SessionCreateResult {
                        session_id: session.id().clone(),
                        agent_preset: session.header().agent_preset.clone(),
                    },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("session.create: agent creation failed: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
    async fn session_rename(
        &self,
        request: RpcRequest<crate::api::sessions::SessionRenameRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let session_id = request.payload.session_id.clone();
        let resolved = self.resolver.resolve(&session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        let Some(titles) = self
            .ctx
            .get_typed::<Arc<dsh_session_title::SessionTitleService>>("sessionTitle", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message:
                        "renaming is unavailable: this deployment mounts no session-title service"
                            .to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        match titles.rename(agent.session(), &request.payload.title) {
            Ok(snapshot) => ok(
                request.rpc_id,
                crate::api::sessions::SessionRenameResult {
                    title: snapshot.title,
                    seq: snapshot.event_seq as i64,
                },
            ),
            Err(dsh_session_title::RenameFailure::Invalid(error)) => err(
                request.rpc_id,
                RpcError::TitleInvalid(RpcErrorBody {
                    message: error.to_string(),
                    details: crate::api::rpc::SessionIdDetails {
                        session_id: session_id.to_string(),
                    },
                }),
            ),
            Err(dsh_session_title::RenameFailure::Error(error)) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("failed to rename session \"{session_id}\": {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    async fn session_cancel(
        &self,
        request: RpcRequest<crate::api::sessions::SessionRefRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(agents) = self.agents() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.cancel: the agents service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(agent) = agents.get(&request.payload.session_id) else {
            return err(
                request.rpc_id,
                RpcError::SessionNotFound(RpcErrorBody {
                    message: format!(
                        "session \"{}\" not found (not attached)",
                        request.payload.session_id
                    ),
                    details: crate::api::rpc::SessionIdDetails {
                        session_id: request.payload.session_id.to_string(),
                    },
                }),
            );
        };
        if crate::agent_lookup::has_api_remote_subagent_owner(
            &self.ctx,
            agent.session().header(),
            Some(&agent),
        ) {
            return err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: format!(
                        "session \"{}\" is owned by subagent routing",
                        request.payload.session_id
                    ),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "use subagent delivery for this child session".to_string(),
                    },
                }),
            );
        }
        agent.cancel(
            dsh_session::AgentCancelCause::User,
            Some(&dsh_agent::CancelOptions { keep_inbox: true }),
        );
        ok(
            request.rpc_id,
            crate::api::sessions::AcceptedResult { accepted: true },
        )
    }
    /// The message-aligned history window (TS `paginate`): page boundaries
    /// align to append-origin message boundaries, never cut mid-message.
    /// Model-only replacement copies consume no `maxMessages` counting.
    fn paginate(
        events: &[dsh_session::SessionEvent],
        before_seq: Option<i64>,
        max_messages: u64,
    ) -> (Vec<dsh_session::SessionEvent>, bool) {
        const MESSAGE_TYPES: [&str; 2] = ["user/message", "assistant/message"];
        let window: Vec<dsh_session::SessionEvent> = match before_seq {
            None => events.to_vec(),
            Some(before) => events
                .iter()
                .filter(|event| (event.seq as i64) < before)
                .cloned()
                .collect(),
        };
        let mut count: u64 = 0;
        let mut cut: u64 = 0;
        for event in window.iter().rev() {
            if !MESSAGE_TYPES.contains(&event.type_.as_str())
                || !event.surface_op.as_ref().is_none_or(|op| op.is_append())
            {
                continue;
            }
            count += 1;
            let group_start = match &event.source_event_seqs {
                Some(sources) if !sources.is_empty() => sources
                    .iter()
                    .copied()
                    .min()
                    .unwrap_or(event.seq)
                    .min(event.seq),
                _ => event.seq,
            };
            if count >= max_messages {
                cut = group_start;
                break;
            }
        }
        let page: Vec<dsh_session::SessionEvent> = window
            .into_iter()
            .filter(|event| event.seq >= cut)
            .collect();
        let has_more = cut > 0;
        (page, has_more)
    }

    async fn session_history(
        &self,
        request: RpcRequest<crate::api::sessions::SessionHistoryRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::HistoryEntry;

        let session_id = request.payload.session_id.clone();
        // The source: an attached session is the live object; a detached
        // one is a frozen persistence inspection (TS `historySourceFor`).
        let events: Vec<dsh_session::SessionEvent> =
            match self.sessions().and_then(|store| store.get(&session_id)) {
                Some(session) => session.events().to_vec(),
                None => {
                    let Some(persistence) = self
                        .ctx
                        .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                            "sessionPersistence",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return err(
                            request.rpc_id,
                            RpcError::SessionNotFound(RpcErrorBody {
                                message: format!("session \"{session_id}\" not found"),
                                details: crate::api::rpc::SessionIdDetails {
                                    session_id: session_id.to_string(),
                                },
                            }),
                        );
                    };
                    match persistence.inspect(&session_id).await {
                        Ok(inspection) => inspection.events,
                        Err(_) => {
                            return err(
                                request.rpc_id,
                                RpcError::SessionNotFound(RpcErrorBody {
                                    message: format!("session \"{session_id}\" not found"),
                                    details: crate::api::rpc::SessionIdDetails {
                                        session_id: session_id.to_string(),
                                    },
                                }),
                            );
                        }
                    }
                }
            };
        const DEFAULT_MAX_MESSAGES: u64 = 100;
        let (page_events, has_more) = Self::paginate(
            &events,
            request.payload.before_seq,
            request.payload.max_messages.unwrap_or(DEFAULT_MAX_MESSAGES),
        );
        // The host-computed render intent arrives with the presenter
        // milestone (TS `viewFor`); entries carry the raw event for now.
        let page: Vec<HistoryEntry> = page_events
            .into_iter()
            .map(|event| HistoryEntry { event, view: None })
            .collect();
        ok(
            request.rpc_id,
            crate::api::sessions::SessionHistoryResult {
                events: page,
                has_more,
                projections: None,
            },
        )
    }
    /// The current model selection for one live agent (picked tier, else
    /// the host default; the logged-request tier arrives with the
    /// request-header milestone).
    fn selection_for(&self, agent: &Arc<dyn Agent>) -> crate::api::sessions::ModelSelection {
        let session_id = agent.id().clone();
        if let Some(picked) = self.selections.lock().get(&session_id) {
            return picked.clone();
        }
        (self.defaults.default_model_selection)()
    }

    async fn session_models(
        &self,
        request: RpcRequest<crate::api::sessions::SessionRefRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let resolved = self.resolver.resolve(&request.payload.session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.models: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let current = self.selection_for(&agent);
        let catalog = Self::build_model_catalog(&runtime).await;
        let routable = runtime
            .list_providers()
            .iter()
            .any(|provider| provider.id == current.provider);
        ok(
            request.rpc_id,
            crate::api::sessions::SessionModels {
                current,
                routable,
                groups: catalog.groups,
                failures: catalog.failures,
            },
        )
    }

    async fn session_select_model(
        &self,
        request: RpcRequest<crate::api::sessions::SessionSelectModelRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let resolved = self.resolver.resolve(&request.payload.session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.selectModel: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let resolved_config = runtime
            .resolve_call_config(
                &dsh_llm::LlmCallConfig {
                    provider: request.payload.provider.clone(),
                    model: request.payload.model.clone(),
                    reasoning_effort: request
                        .payload
                        .reasoning_effort
                        .clone()
                        .map(|id| dsh_llm::ReasoningEffortId::new(id)),
                    ..Default::default()
                },
                None,
            )
            .await;
        let selected = match resolved_config {
            Ok(config) => crate::api::sessions::ModelSelection {
                provider: config.provider,
                model: config.model,
                reasoning_effort: config.reasoning_effort.map(|id| id.to_string()),
            },
            Err(error) => {
                return err(
                    request.rpc_id,
                    RpcError::ModelUnavailable(RpcErrorBody {
                        message: error.to_string(),
                        details: crate::api::rpc::ModelUnavailableDetails {
                            provider: request.payload.provider,
                            model: request.payload.model,
                        },
                    }),
                );
            }
        };
        // The image-admission fence arrives with the attachment milestone
        // (TS checks pending inbox images against input modalities).
        self.selections
            .lock()
            .insert(agent.id().clone(), selected.clone());
        ok(
            request.rpc_id,
            crate::api::sessions::SessionSelectModelResult { selected },
        )
    }
}

/// Mint a fresh correlation id (time + process-local counter; uniqueness,
/// not cryptographic strength).
fn fresh_id_proxy_counter() -> &'static std::sync::atomic::AtomicU64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &COUNTER
}

impl ApiProxyService {
    fn fresh_id() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let counter = fresh_id_proxy_counter().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{nanos:x}-{counter:x}")
    }

    async fn session_fork(
        &self,
        request: RpcRequest<crate::api::sessions::SessionForkRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let session_id = request.payload.session_id.clone();
        // The source: attached session or frozen persistence inspection
        // (TS `readSessionState`).
        let (header, events): (dsh_session::SessionHeader, Vec<dsh_session::SessionEvent>) =
            match self.sessions().and_then(|store| store.get(&session_id)) {
                Some(session) => (session.header().clone(), session.events().to_vec()),
                None => {
                    let Some(persistence) = self
                        .ctx
                        .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                            "sessionPersistence",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return err(
                            request.rpc_id,
                            RpcError::SessionNotFound(RpcErrorBody {
                                message: format!("session \"{session_id}\" not found"),
                                details: crate::api::rpc::SessionIdDetails {
                                    session_id: session_id.to_string(),
                                },
                            }),
                        );
                    };
                    match persistence.inspect(&session_id).await {
                        Ok(inspection) => (inspection.meta, inspection.events),
                        Err(_) => {
                            return err(
                                request.rpc_id,
                                RpcError::SessionNotFound(RpcErrorBody {
                                    message: format!("session \"{session_id}\" not found"),
                                    details: crate::api::rpc::SessionIdDetails {
                                        session_id: session_id.to_string(),
                                    },
                                }),
                            );
                        }
                    }
                }
            };
        let last_seq = events.last().map(|event| event.seq as i64).unwrap_or(-1);
        let at_seq = request.payload.at_seq;
        // An in-log anchor belongs to the turn containing it; omitted and
        // past-end anchors retain the last-completed-turn shortcut.
        let anchored_boundary = at_seq.and_then(|at| {
            events
                .iter()
                .find(|event| event.type_ == "turn/end" && (event.seq as i64) >= at)
                .map(|event| event.seq)
        });
        let boundary = match anchored_boundary {
            Some(seq) => Some(seq),
            None if at_seq.is_none_or(|at| at > last_seq) => events
                .iter()
                .rev()
                .find(|event| event.type_ == "turn/end")
                .map(|event| event.seq),
            None => None,
        };
        let Some(boundary) = boundary else {
            return err(
                request.rpc_id,
                RpcError::ForkUnavailable(RpcErrorBody {
                    message: match at_seq {
                        Some(at) if at <= last_seq => format!(
                            "session \"{session_id}\" has not completed the turn containing event {at}"
                        ),
                        _ => format!("session \"{session_id}\" has no completed turn to fork from"),
                    },
                    details: crate::api::rpc::SessionIdDetails {
                        session_id: session_id.to_string(),
                    },
                }),
            );
        };
        // Extend the cut through trailing out-of-band appends up to the next
        // turn/start.
        let mut cut = boundary + 1;
        while (cut as usize) < events.len() && events[cut as usize].type_ != "turn/start" {
            cut += 1;
        }
        let child_id = dsh_session::session_id(format!("session-{}", Self::fresh_id()));
        let seed: Vec<dsh_session::SessionEvent> = events[..cut as usize].to_vec();
        let Some(agents) = self.agents() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.fork: the agents service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let agent_options = {
            let selection = (self.defaults.default_model_selection)();
            dsh_agent::AgentOptions {
                provider: Some(selection.provider),
                model: Some(selection.model),
                ..Default::default()
            }
        };
        let mut meta = dsh_session::CreateSessionMeta {
            cwd: header.cwd.clone(),
            parent_session: Some(session_id.clone()),
            seed_length: Some(cut as u64),
            agent_preset: header.agent_preset.clone(),
            ..Default::default()
        };
        let _ = &mut meta;
        match agents
            .create(dsh_agent::CreateAgentOptions {
                session_id: Some(child_id.clone()),
                seed: Some(seed),
                meta: Some(meta),
                agent_options: Some(agent_options),
                ..Default::default()
            })
            .await
        {
            Ok(_handle) => {
                // Workspace attachment follows the source (TS forkWorkspace);
                // it arrives with the workspace-attach milestone.
                ok(
                    request.rpc_id,
                    crate::api::sessions::SessionForkResult {
                        session_id: child_id,
                    },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("failed to fork session \"{session_id}\": {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }
    async fn session_update_queue(
        &self,
        request: RpcRequest<crate::api::sessions::SessionUpdateQueueRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::QueueAction;

        let item_id = request.payload.item_id.clone();
        if let QueueAction::Edit { content } = &request.payload.action {
            if content
                .iter()
                .any(|block| !matches!(block, dsh_llm::ContentBlock::Text { .. }))
            {
                return err(
                    request.rpc_id,
                    RpcError::AttachmentError(RpcErrorBody {
                        message: "queue edits accept text content only".to_string(),
                        details: crate::api::rpc::ReasonDetails {
                            reason: "QUEUE_EDIT_NON_TEXT".to_string(),
                        },
                    }),
                );
            }
        }
        let Some(agents) = self.agents() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.updateQueue: the agents service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let Some(agent) = agents.get(&request.payload.session_id) else {
            return err(
                request.rpc_id,
                RpcError::QueueItemNotFound(RpcErrorBody {
                    message: "queued item is no longer pending".to_string(),
                    details: crate::api::rpc::ItemIdDetails {
                        item_id: item_id.to_string(),
                    },
                }),
            );
        };
        if crate::agent_lookup::has_api_remote_subagent_owner(
            &self.ctx,
            agent.session().header(),
            Some(&agent),
        ) {
            return err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: format!(
                        "session \"{}\" is owned by subagent routing",
                        request.payload.session_id
                    ),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "use subagent delivery for this child session".to_string(),
                    },
                }),
            );
        }
        let inbox = agent.inbox();
        let in_turn = inbox
            .next_turn()
            .iter()
            .any(|message| &message.id == &item_id);
        let in_step = inbox
            .next_step()
            .iter()
            .any(|message| &message.id == &item_id);
        if !in_turn && !in_step {
            return err(
                request.rpc_id,
                RpcError::QueueItemNotFound(RpcErrorBody {
                    message: "queued item is no longer pending".to_string(),
                    details: crate::api::rpc::ItemIdDetails {
                        item_id: item_id.to_string(),
                    },
                }),
            );
        }
        let message = if in_turn {
            inbox
                .next_turn()
                .into_iter()
                .find(|message| &message.id == &item_id)
        } else {
            inbox
                .next_step()
                .into_iter()
                .find(|message| &message.id == &item_id)
        };
        let Some(message) = message else {
            return err(
                request.rpc_id,
                RpcError::QueueItemNotFound(RpcErrorBody {
                    message: "queued item is no longer pending".to_string(),
                    details: crate::api::rpc::ItemIdDetails {
                        item_id: item_id.to_string(),
                    },
                }),
            );
        };
        if matches!(request.payload.action, QueueAction::Steer)
            && (!in_turn || agent.status() != dsh_agent::AgentStatus::Running)
        {
            return err(
                request.rpc_id,
                RpcError::SteerUnavailable(RpcErrorBody {
                    message: "current turn no longer accepts steering".to_string(),
                    details: crate::api::rpc::ItemIdDetails {
                        item_id: item_id.to_string(),
                    },
                }),
            );
        }
        match request.payload.action {
            QueueAction::Edit { content } => {
                let mut edited = message.clone();
                edited.content = content;
                let _ = inbox.replace(&item_id, edited);
            }
            QueueAction::Remove => {
                let _ = inbox.remove(&item_id);
            }
            QueueAction::Steer => {
                let _ = inbox.remove(&item_id);
                agent.steer(message);
            }
        }
        ok(
            request.rpc_id,
            crate::api::sessions::AcceptedResult { accepted: true },
        )
    }
    async fn session_prompt(
        &self,
        request: RpcRequest<crate::api::sessions::SessionPromptRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::{PromptContentPart, PromptMode};

        // The browser zone is validated and canonicalized up front (TS
        // `canonicalClientTimeZone`).
        let canonical_time_zone = match &request.payload.client_time_zone {
            None => None,
            Some(zone) => match dsh_time_context::timestamp::canonical_time_zone(zone) {
                Ok(canonical) => Some(canonical),
                Err(_) => {
                    return err(
                        request.rpc_id,
                        RpcError::InvalidTimeZone(RpcErrorBody {
                            message:
                                "clientTimeZone must be UTC or a valid IANA Area/Location name"
                                    .to_string(),
                            details: crate::api::rpc::ValueDetails {
                                value: zone.clone(),
                            },
                        }),
                    );
                }
            },
        };
        let resolved = self.resolver.resolve(&request.payload.session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        // Image admission and durable promotion arrive with the attachment
        // milestone; image parts are refused for now.
        if request
            .payload
            .content
            .iter()
            .any(|part| matches!(part, PromptContentPart::Image { .. }))
        {
            return err(
                request.rpc_id,
                RpcError::AttachmentError(RpcErrorBody {
                    message: "image admission is not implemented in the Rust composition yet"
                        .to_string(),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "MODEL_DOES_NOT_SUPPORT_IMAGES".to_string(),
                    },
                }),
            );
        }
        let content: Vec<dsh_llm::ContentBlock> = request
            .payload
            .content
            .iter()
            .map(|part| match part {
                PromptContentPart::Text { text } => {
                    dsh_llm::ContentBlock::Text { text: text.clone() }
                }
                PromptContentPart::Image { .. } => unreachable!("image parts refused above"),
            })
            .collect();
        // Request identity and optional browser zone ride the exact durable
        // user message.
        let source = dsh_llm::MessageSource::User {
            rpc_id: Some(request.rpc_id.to_string()),
            client_time_zone: canonical_time_zone,
        };
        let message = dsh_llm::create_user_message(content, source);
        match request.payload.mode {
            PromptMode::Steer => agent.steer(message),
            PromptMode::Queue => agent.followup(message),
        }
        ok(
            request.rpc_id,
            crate::api::sessions::SessionPromptResult {
                accepted: true,
                command: None,
            },
        )
    }
    /// Extract the first image reference matching the attachment id from any
    /// event's message content (TS `referencedImage`).
    fn referenced_image(
        events: &[dsh_session::SessionEvent],
        attachment_id: &str,
    ) -> Option<dsh_attachment::ImageAttachmentRef> {
        fn scan(
            value: &serde_json::Value,
            attachment_id: &str,
        ) -> Option<dsh_attachment::ImageAttachmentRef> {
            match value {
                serde_json::Value::Object(object) => {
                    if object.get("type").and_then(serde_json::Value::as_str) == Some("image") {
                        if let Some(reference) = object.get("attachment") {
                            if reference
                                .get("attachmentId")
                                .and_then(serde_json::Value::as_str)
                                == Some(attachment_id)
                            {
                                if let Ok(reference) =
                                    serde_json::from_value::<dsh_attachment::ImageAttachmentRef>(
                                        reference.clone(),
                                    )
                                {
                                    return Some(reference);
                                }
                            }
                        }
                    }
                    object.values().find_map(|value| scan(value, attachment_id))
                }
                serde_json::Value::Array(array) => {
                    array.iter().find_map(|value| scan(value, attachment_id))
                }
                _ => None,
            }
        }
        events
            .iter()
            .find_map(|event| scan(&event.data, attachment_id))
    }

    async fn session_attachment(
        &self,
        request: RpcRequest<crate::api::sessions::SessionAttachmentRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let session_id = request.payload.session_id.clone();
        let attachment_id = request.payload.attachment_id.to_string();
        let events: Vec<dsh_session::SessionEvent> =
            match self.sessions().and_then(|store| store.get(&session_id)) {
                Some(session) => session.events().to_vec(),
                None => {
                    let Some(persistence) = self
                        .ctx
                        .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                            "sessionPersistence",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return err(
                            request.rpc_id,
                            RpcError::SessionNotFound(RpcErrorBody {
                                message: format!("session \"{session_id}\" not found"),
                                details: crate::api::rpc::SessionIdDetails {
                                    session_id: session_id.to_string(),
                                },
                            }),
                        );
                    };
                    match persistence.inspect(&session_id).await {
                        Ok(inspection) => inspection.events,
                        Err(_) => {
                            return err(
                                request.rpc_id,
                                RpcError::SessionNotFound(RpcErrorBody {
                                    message: format!("session \"{session_id}\" not found"),
                                    details: crate::api::rpc::SessionIdDetails {
                                        session_id: session_id.to_string(),
                                    },
                                }),
                            );
                        }
                    }
                }
            };
        let Some(reference) = Self::referenced_image(&events, &attachment_id) else {
            return err(
                request.rpc_id,
                RpcError::AttachmentError(RpcErrorBody {
                    message: "Image is not referenced by this session.".to_string(),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "ATTACHMENT_NOT_REFERENCED".to_string(),
                    },
                }),
            );
        };
        let Some(store) = self
            .ctx
            .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.attachment: the attachments service is not composed"
                        .to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        match store.read_image(&reference, None).await {
            Ok(stored) => {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&stored.data);
                ok(
                    request.rpc_id,
                    crate::api::sessions::SessionAttachmentResult {
                        attachment: stored.reference,
                        data,
                    },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::AttachmentError(RpcErrorBody {
                    message: error.to_string(),
                    details: crate::api::rpc::ReasonDetails {
                        reason: error.code.clone(),
                    },
                }),
            ),
        }
    }
    async fn session_search(
        &self,
        request: RpcRequest<crate::api::sessions::SessionSearchRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::{SessionSearchItem, SessionSummary};

        const RESULT_LIMIT: usize = 20;
        const PROVIDER_CALL_LIMIT: usize = 8;
        const SNIPPET_MAX_CODE_POINTS: usize = 120;

        let cancelled = || {
            err::<serde_json::Value>(
                request.rpc_id.clone(),
                RpcError::Cancelled(RpcErrorBody {
                    message: "session search was aborted".to_string(),
                    details: EmptyDetails {},
                }),
            )
        };
        if signal.aborted() {
            return cancelled();
        }
        let Some(engine) = self
            .ctx
            .get_typed::<Arc<dsh_session_query::SessionQueryEngine>>("sessionQuery", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session search is unavailable: this deployment does not mount dsh-session-query".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        // The visible set is the authorization boundary (attached + cold
        // summaries without the search work).
        let visible: Vec<SessionSummary> = match self
            .session_list(RpcRequest {
                rpc_id: request.rpc_id.clone(),
                payload: crate::api::sessions::SessionListRequest { cursor: None },
            })
            .await
            .result
        {
            crate::api::rpc::RpcResult::Ok { value, .. } => {
                let value: crate::api::sessions::SessionListResult =
                    serde_json::from_value(value).expect("session list result");
                value.items
            }
            crate::api::rpc::RpcResult::Err { error, .. } => {
                return err(request.rpc_id, error);
            }
        };
        if signal.aborted() {
            return cancelled();
        }
        if visible.is_empty() {
            return ok(
                request.rpc_id,
                crate::api::sessions::SessionSearchResult {
                    items: Vec::new(),
                    has_more: false,
                },
            );
        }
        let visible_ids: std::collections::HashSet<String> = visible
            .iter()
            .map(|item| item.session_id.to_string())
            .collect();
        let mut authorized: Vec<SessionSearchItem> = Vec::new();
        let mut accepted_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_cursors: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut cursor: Option<dsh_session_query::SessionSearchCursor> = None;
        let mut provider_call_count = 0;
        let mut provider_page_limit = RESULT_LIMIT;
        while authorized.len() <= RESULT_LIMIT {
            if signal.aborted() {
                return cancelled();
            }
            if provider_call_count >= PROVIDER_CALL_LIMIT {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!(
                            "session search provider exceeded the {PROVIDER_CALL_LIMIT}-call work budget"
                        ),
                        details: EmptyDetails {},
                    }),
                );
            }
            provider_call_count += 1;
            let abort_flag = signal.clone();
            let page = engine
                .search_sessions(
                    &dsh_session_query::SessionSearchRequest {
                        query: request.payload.query.clone(),
                        session_filters: None,
                        event_filters: Some(vec![
                            dsh_session_query::SessionEventResultFilter::Type {
                                values: vec![
                                    "user/message".to_string(),
                                    "assistant/message".to_string(),
                                ],
                            },
                            dsh_session_query::SessionEventResultFilter::Surface {
                                values: vec![dsh_session_query::SessionEventSurface::Current],
                            },
                        ]),
                        limit: Some(provider_page_limit as u64),
                        cursor: cursor.clone(),
                    },
                    Some(&dsh_session_query::SessionSearchExecContext {
                        signal: Some(Arc::new(move || abort_flag.aborted())),
                    }),
                )
                .await;
            let page = match page {
                Ok(page) => page,
                Err(error) => {
                    if signal.aborted() {
                        return cancelled();
                    }
                    if cursor.is_none()
                        && error.code
                            == dsh_session_query::SessionQueryErrorCode::SessionQueryInvalidLimit
                        && provider_page_limit > 1
                    {
                        provider_page_limit = (provider_page_limit / 2).max(1);
                        continue;
                    }
                    if cursor.is_some()
                        && error.code
                            == dsh_session_query::SessionQueryErrorCode::SessionQueryStaleCursor
                    {
                        authorized.clear();
                        accepted_ids.clear();
                        seen_cursors.clear();
                        cursor = None;
                        continue;
                    }
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!("session search failed: {error}"),
                            details: EmptyDetails {},
                        }),
                    );
                }
            };
            if signal.aborted() {
                return cancelled();
            }
            if page.items.len() > provider_page_limit {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!(
                            "session search provider returned {} items; maximum is {provider_page_limit}",
                            page.items.len()
                        ),
                        details: EmptyDetails {},
                    }),
                );
            }
            for hit in &page.items {
                if authorized.len() > RESULT_LIMIT {
                    continue;
                }
                let header_id = hit.record.header.id.to_string();
                let best = &hit.best_match;
                if !visible_ids.contains(&header_id)
                    || best.session_id.to_string() != header_id
                    || best.surface != dsh_session_query::SessionEventSurface::Current
                    || (best.type_ != "user/message" && best.type_ != "assistant/message")
                    || accepted_ids.contains(&header_id)
                {
                    continue;
                }
                let snippet: String = best.snippet.chars().take(SNIPPET_MAX_CODE_POINTS).collect();
                accepted_ids.insert(header_id.clone());
                authorized.push(SessionSearchItem {
                    session_id: dsh_session::session_id(header_id),
                    snippet,
                });
            }
            let next_cursor = page.next_cursor.clone();
            if let Some(next) = &next_cursor {
                if !seen_cursors.insert(next.to_string()) {
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: "session search provider repeated a continuation cursor"
                                .to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                }
            }
            if authorized.len() > RESULT_LIMIT || next_cursor.is_none() {
                break;
            }
            cursor = next_cursor;
        }
        ok(
            request.rpc_id,
            crate::api::sessions::SessionSearchResult {
                items: authorized,
                has_more: false,
            },
        )
    }
    fn subagents(&self) -> Option<Arc<dsh_subagent::SubagentRuntime>> {
        self.ctx
            .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn subagents_absent() -> RpcError {
        RpcError::Internal(RpcErrorBody {
            message: "subagent service is absent: the host composition does not mount dsh-subagent"
                .to_string(),
            details: EmptyDetails {},
        })
    }

    /// Project one domain listing entry into the wire view.
    fn wire_subagent_entry(
        entry: &dsh_subagent::SubagentListEntry,
        activity: Option<&str>,
    ) -> crate::api::subagents::SubagentListEntry {
        use crate::api::subagents::{SubagentActivity, SubagentDiagnosticReason, SubagentMode};

        match entry {
            dsh_subagent::SubagentListEntry::Child {
                id,
                has_children,
                identity,
                ..
            } => {
                let (mode, label) = match identity {
                    dsh_subagent::SubagentIdentityProjection::OneShot { label, .. } => {
                        (SubagentMode::OneShot, label.clone())
                    }
                    dsh_subagent::SubagentIdentityProjection::Continuable { label, .. } => {
                        (SubagentMode::Continuable, Some(label.clone()))
                    }
                };
                crate::api::subagents::SubagentListEntry::Child {
                    id: id.clone(),
                    activity: match activity {
                        Some("running") => SubagentActivity::Running,
                        _ => SubagentActivity::Inactive,
                    },
                    has_children: *has_children,
                    mode,
                    label,
                }
            }
            dsh_subagent::SubagentListEntry::Diagnostic { id, reason } => {
                crate::api::subagents::SubagentListEntry::Diagnostic {
                    id: id.clone(),
                    reason: match reason.as_str() {
                        "corrupt" => SubagentDiagnosticReason::Corrupt,
                        "unsupported" => SubagentDiagnosticReason::Unsupported,
                        _ => SubagentDiagnosticReason::Unavailable,
                    },
                }
            }
        }
    }

    async fn subagent_list(
        &self,
        request: RpcRequest<crate::api::subagents::SubagentListRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let Some(runtime) = self.subagents() else {
            return err(request.rpc_id, Self::subagents_absent());
        };
        let abort_flag = signal.clone();
        let signal_ref: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || abort_flag.aborted());
        match runtime
            .list_children(&request.payload.parent_session_id, Some(&signal_ref))
            .await
        {
            Ok(entries) => {
                let entries: Vec<crate::api::subagents::SubagentListEntry> = entries
                    .iter()
                    .map(|entry| {
                        let activity = match entry {
                            dsh_subagent::SubagentListEntry::Child { id, .. } => self
                                .agents()
                                .and_then(|registry| registry.get(id))
                                .map(|agent| {
                                    if agent.status() == dsh_agent::AgentStatus::Running {
                                        "running"
                                    } else {
                                        "inactive"
                                    }
                                }),
                            _ => None,
                        };
                        Self::wire_subagent_entry(entry, activity)
                    })
                    .collect();
                let parent_available = self
                    .agents()
                    .and_then(|registry| registry.get(&request.payload.parent_session_id))
                    .is_some();
                ok(
                    request.rpc_id,
                    crate::api::subagents::SubagentCatalog {
                        entries,
                        parent_available,
                    },
                )
            }
            Err(error) => {
                if signal.aborted() || error.code == "CANCELLED" {
                    return err(
                        request.rpc_id,
                        RpcError::Cancelled(RpcErrorBody {
                            message: "subagent catalog read was cancelled".to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                }
                err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: "subagent catalog read failed".to_string(),
                        details: EmptyDetails {},
                    }),
                )
            }
        }
    }

    async fn subagent_history(
        &self,
        request: RpcRequest<crate::api::subagents::SubagentHistoryRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::HistoryEntry;

        let child_id = request.payload.child_session_id.clone();
        let parent_id = request.payload.parent_session_id.clone();
        // The generic-history data plane: attached child or cold inspection.
        let (header, events): (dsh_session::SessionHeader, Vec<dsh_session::SessionEvent>) =
            match self.sessions().and_then(|store| store.get(&child_id)) {
                Some(session) => (session.header().clone(), session.events().to_vec()),
                None => {
                    let Some(persistence) = self
                        .ctx
                        .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                            "sessionPersistence",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return err(
                            request.rpc_id,
                            RpcError::SubagentNotFound(RpcErrorBody {
                                message: "subagent disappeared during history read".to_string(),
                                details: crate::api::rpc::SubagentPairDetails {
                                    parent_session_id: parent_id.to_string(),
                                    child_session_id: child_id.to_string(),
                                },
                            }),
                        );
                    };
                    match persistence.inspect(&child_id).await {
                        Ok(inspection) => (inspection.meta, inspection.events),
                        Err(_) => {
                            return err(
                                request.rpc_id,
                                RpcError::SubagentNotFound(RpcErrorBody {
                                    message: "subagent disappeared during history read".to_string(),
                                    details: crate::api::rpc::SubagentPairDetails {
                                        parent_session_id: parent_id.to_string(),
                                        child_session_id: child_id.to_string(),
                                    },
                                }),
                            );
                        }
                    }
                }
            };
        if signal.aborted() {
            return err(
                request.rpc_id,
                RpcError::Cancelled(RpcErrorBody {
                    message: "subagent history read was cancelled".to_string(),
                    details: EmptyDetails {},
                }),
            );
        }
        if header.parent_session.as_ref() != Some(&parent_id) {
            return err(
                request.rpc_id,
                RpcError::SubagentUnauthorized(RpcErrorBody {
                    message: "subagent parent changed during history read".to_string(),
                    details: crate::api::rpc::ChildSessionIdDetails {
                        child_session_id: child_id.to_string(),
                    },
                }),
            );
        }
        const DEFAULT_MAX_MESSAGES: u64 = 100;
        let (page_events, has_more) = Self::paginate(
            &events,
            request.payload.before_seq,
            request.payload.max_messages.unwrap_or(DEFAULT_MAX_MESSAGES),
        );
        let page: Vec<HistoryEntry> = page_events
            .into_iter()
            .map(|event| HistoryEntry { event, view: None })
            .collect();
        ok(
            request.rpc_id,
            crate::api::subagents::SubagentHistoryResult {
                events: page,
                has_more,
                projections: None,
            },
        )
    }

    async fn subagent_prompt(
        &self,
        request: RpcRequest<crate::api::subagents::SubagentPromptRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::subagents::SubagentPromptReceipt;

        let canonical_time_zone = match &request.payload.client_time_zone {
            None => None,
            Some(zone) => match dsh_time_context::timestamp::canonical_time_zone(zone) {
                Ok(canonical) => Some(canonical),
                Err(_) => {
                    return err(
                        request.rpc_id,
                        RpcError::InvalidTimeZone(RpcErrorBody {
                            message:
                                "clientTimeZone must be UTC or a valid IANA Area/Location name"
                                    .to_string(),
                            details: crate::api::rpc::ValueDetails {
                                value: zone.clone(),
                            },
                        }),
                    );
                }
            },
        };
        let Some(runtime) = self.subagents() else {
            return err(request.rpc_id, Self::subagents_absent());
        };
        let parent_id = request.payload.parent_session_id.clone();
        let child_id = request.payload.child_session_id.clone();
        let Some(parent) = self.agents().and_then(|registry| registry.get(&parent_id)) else {
            return err(
                request.rpc_id,
                RpcError::SubagentParentUnavailable(RpcErrorBody {
                    message: format!("parent session \"{parent_id}\" is not live"),
                    details: crate::api::rpc::ParentSessionIdDetails {
                        parent_session_id: parent_id.to_string(),
                    },
                }),
            );
        };
        let source = dsh_llm::MessageSource::User {
            rpc_id: Some(request.rpc_id.to_string()),
            client_time_zone: canonical_time_zone,
        };
        let abort_flag = signal.clone();
        let options = dsh_subagent::SubagentFollowupOptions {
            source,
            signal: Arc::new(move || abort_flag.aborted()),
        };
        match runtime
            .followup(parent, &child_id, &request.payload.content, options)
            .await
        {
            Ok(message_id) => ok(request.rpc_id, SubagentPromptReceipt { message_id }),
            Err(error) => {
                if signal.aborted() || error.code == "CANCELLED" {
                    return err(
                        request.rpc_id,
                        RpcError::Cancelled(RpcErrorBody {
                            message: "subagent prompt was cancelled".to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                }
                err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!("subagent prompt failed: {error}"),
                        details: EmptyDetails {},
                    }),
                )
            }
        }
    }

    async fn subagent_interrupt(
        &self,
        request: RpcRequest<crate::api::subagents::SubagentInterruptRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::subagents::SubagentInterruptReceipt;

        let Some(runtime) = self.subagents() else {
            return err(request.rpc_id, Self::subagents_absent());
        };
        let authority = dsh_subagent::SubagentInterruptAuthority::User {
            parent_session_id: request.payload.parent_session_id.clone(),
        };
        match runtime.interrupt(&request.payload.child_session_id, &authority) {
            Ok(()) => ok(request.rpc_id, SubagentInterruptReceipt { accepted: true }),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("subagent interrupt failed: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    // ---- agentPreset domain -------------------------------------------------

    /// Open one Host-resolved target and map native failures onto the wire
    /// vocabulary (TS `openTarget`).
    async fn open_target(
        &self,
        _rpc_id: RpcId,
        path: String,
        signal: AbortSignal,
    ) -> Result<(), RpcError> {
        let open = match &self.defaults.open_path {
            Some(open) => open.clone(),
            None => {
                // Fallback to the platform opener (TS `openNativePath`).
                return match crate::native_path_opener::open_native_path(
                    &path,
                    None,
                    &crate::native_path_opener::PathOpenerInternals::default(),
                )
                .await
                {
                    Ok(()) => Ok(()),
                    Err(error) => Err(RpcError::Internal(RpcErrorBody {
                        message: format!("path open failed: {error}"),
                        details: EmptyDetails {},
                    })),
                };
            }
        };
        match open(path, signal.clone()).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if signal.aborted() {
                    Err(RpcError::Cancelled(RpcErrorBody {
                        message: "path open was aborted".to_string(),
                        details: EmptyDetails {},
                    }))
                } else {
                    Err(RpcError::Internal(RpcErrorBody {
                        message: format!("path open failed: {error}"),
                        details: EmptyDetails {},
                    }))
                }
            }
        }
    }

    /// `agentPreset.list`: every preset the deployment supplies, in
    /// root-precedence order. A deployment with no roster answers with an
    /// empty list (composing no presets is a valid deployment).
    async fn agent_preset_list(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(presets) = self.agent_presets() else {
            return ok(
                request.rpc_id,
                crate::api::agent_presets::AgentPresetListResult {
                    presets: Vec::new(),
                    authorable: false,
                    has_document: false,
                },
            );
        };
        let default_id = presets.default_id();
        match presets.list().await {
            Ok(roster) => {
                let entries = roster
                    .into_iter()
                    .map(|preset| {
                        let is_default = preset.id == default_id;
                        crate::api::agent_presets::AgentPresetEntry {
                            id: preset.id,
                            trust: match preset.trust {
                                dsh_agent_presets::PresetTrust::System => {
                                    crate::api::agent_presets::AgentPresetTrust::System
                                }
                                dsh_agent_presets::PresetTrust::User => {
                                    crate::api::agent_presets::AgentPresetTrust::User
                                }
                            },
                            is_default,
                            name: preset.name,
                            description: preset.description,
                            broken: preset.broken,
                        }
                    })
                    .collect();
                ok(
                    request.rpc_id,
                    crate::api::agent_presets::AgentPresetListResult {
                        presets: entries,
                        authorable: presets.authorable(),
                        has_document: self.can_open_paths(),
                    },
                )
            }
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("agent preset roster read failed: {error}"),
                    details: EmptyDetails {},
                }),
            ),
        }
    }

    /// `agentPreset.select`: recompose a blank session's agent from a
    /// different preset, serialized per session (TS `agentPresets.select`).
    async fn agent_preset_select(
        &self,
        request: RpcRequest<crate::api::agent_presets::AgentPresetSelectRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let session_id = request.payload.session_id.clone();
        let agent_preset = request.payload.agent_preset.clone();
        let Some(presets) = self.agent_presets() else {
            return self.no_roster(rpc_id, &agent_preset);
        };
        let resolved = self.resolver.resolve(&session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(rpc_id, error);
            }
        };
        let chains = self.preset_switches.clone();
        let swap = {
            let session_for_swap = agent.session().clone();
            let agent_for_swap = agent.clone();
            let rpc_id_for_swap = rpc_id.clone();
            let session_id_for_swap = session_id.clone();
            let agent_preset_for_swap = agent_preset.clone();
            async move {
                // Re-read inside the queue: an earlier switch may have run,
                // and a conversation may have started, since this request
                // arrived (TS `swap`).
                let started = session_for_swap
                    .events()
                    .iter()
                    .any(|event| event.type_ == "turn/start");
                if started {
                    return Arc::new(err(
                        rpc_id_for_swap,
                        RpcError::AgentPresetLocked(RpcErrorBody {
                            message: format!(
                                "session \"{session_id_for_swap}\" has already started; its agent preset is fixed"
                            ),
                            details: crate::api::rpc::AgentPresetLockedDetails {
                                session_id: session_id_for_swap.to_string(),
                                agent_preset: agent_preset_for_swap,
                            },
                        }),
                    ));
                }
                match presets
                    .recompose(agent_for_swap.ctx(), &agent_preset_for_swap)
                    .await
                {
                    Ok(preset) => {
                        // Recorded only after the swap committed: the log
                        // states what the agent runs, and a rejected mount
                        // leaves the previous composition.
                        if let Err(error) = session_for_swap.append(
                            dsh_agent_presets::AGENT_PRESET_SELECTED,
                            dsh_agent_presets::selected_data(&preset.id),
                            None,
                        ) {
                            return Arc::new(err(
                                rpc_id_for_swap,
                                RpcError::Internal(RpcErrorBody {
                                    message: format!(
                                        "failed to select agent preset \"{agent_preset_for_swap}\": {error}"
                                    ),
                                    details: EmptyDetails {},
                                }),
                            ));
                        }
                        Arc::new(ok(
                            rpc_id_for_swap,
                            crate::api::agent_presets::AgentPresetSelectResult {
                                agent_preset: preset.id,
                            },
                        ))
                    }
                    Err(error) => Arc::new(err(
                        rpc_id_for_swap,
                        RpcError::AgentPresetInvalid(RpcErrorBody {
                            message: error.to_string(),
                            details: crate::api::rpc::AgentPresetReasonDetails {
                                agent_preset: error.preset_id,
                                reason: error.reason,
                            },
                        }),
                    )),
                }
            }
        };
        // The TS chain: `queued.then(swap)` with the map holding the settled
        // tail of every turn (a turn never rejects — every arm returns a
        // response).
        let token = self
            .preset_switch_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let ready: futures::future::Shared<
            BoxFuture<'static, Arc<RpcResponse<serde_json::Value>>>,
        > = futures::future::ready(Arc::new(ok(rpc_id.clone(), serde_json::Value::Null)))
            .boxed()
            .shared();
        let queued = chains
            .lock()
            .get(&session_id)
            .map(|(_token, shared)| shared.clone())
            .unwrap_or_else(|| ready.clone());
        let turn: futures::future::Shared<BoxFuture<'static, Arc<RpcResponse<serde_json::Value>>>> =
            queued.then(|_previous| swap).boxed().shared();
        chains
            .lock()
            .insert(session_id.clone(), (token, turn.clone()));
        let result = (*turn.await).clone();
        // TS finally: remove the settled entry when it is still this turn.
        let still_head = chains
            .lock()
            .get(&session_id)
            .is_some_and(|(head_token, _shared)| *head_token == token);
        if still_head {
            chains.lock().remove(&session_id);
        }
        result
    }

    /// `agentPreset.read`: one preset's composition text for the read-only
    /// viewer (TS `agentPresets.read`).
    async fn agent_preset_read(
        &self,
        request: RpcRequest<crate::api::agent_presets::AgentPresetReadRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let agent_preset = request.payload.agent_preset.clone();
        let Some(presets) = self.agent_presets() else {
            return self.no_roster(rpc_id, &agent_preset);
        };
        match presets.resolve(Some(&agent_preset)).await {
            Ok(preset) => {
                let content = match dsh_agent_presets::read_composition(&preset).await {
                    Ok(content) => content,
                    Err(error) => {
                        return err(
                            rpc_id,
                            RpcError::Internal(RpcErrorBody {
                                message: format!("agent preset \"{agent_preset}\": {error}"),
                                details: EmptyDetails {},
                            }),
                        );
                    }
                };
                ok(
                    rpc_id,
                    crate::api::agent_presets::AgentPresetReadResult {
                        agent_preset: preset.id,
                        trust: match preset.trust {
                            dsh_agent_presets::PresetTrust::System => {
                                crate::api::agent_presets::AgentPresetTrust::System
                            }
                            dsh_agent_presets::PresetTrust::User => {
                                crate::api::agent_presets::AgentPresetTrust::User
                            }
                        },
                        content,
                        name: preset.name,
                        description: preset.description,
                    },
                )
            }
            Err(error) => self.preset_failure_unknown(rpc_id, error),
        }
    }

    /// `agentPreset.copy`: create a locally authored preset by copying an
    /// existing one whole (TS `agentPresets.copy`).
    async fn agent_preset_copy(
        &self,
        request: RpcRequest<crate::api::agent_presets::AgentPresetCopyRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let agent_preset = request.payload.agent_preset.clone();
        let Some(presets) = self.agent_presets() else {
            return self.no_roster(rpc_id, &agent_preset);
        };
        match presets
            .copy(
                &request.payload.from,
                &request.payload.agent_preset,
                request.payload.name.as_deref(),
            )
            .await
        {
            Ok(()) => ok(
                rpc_id,
                crate::api::agent_presets::AgentPresetSelectResult {
                    agent_preset: request.payload.agent_preset,
                },
            ),
            Err(error) => self.preset_error(rpc_id, &agent_preset, error),
        }
    }

    /// `agentPreset.openDocument`: hand one locally authored preset's
    /// directory to the platform opener; shipped presets are refused
    /// (TS `agentPresets.openDocument`).
    async fn agent_preset_open_document(
        &self,
        request: RpcRequest<crate::api::agent_presets::AgentPresetOpenDocumentRequest>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let agent_preset = request.payload.agent_preset.clone();
        let Some(presets) = self.agent_presets() else {
            return self.no_roster(rpc_id, &agent_preset);
        };
        let preset = match presets.resolve(Some(&agent_preset)).await {
            Ok(preset) => preset,
            Err(error) => return self.preset_failure_unknown(rpc_id, error),
        };
        // The shipped install is not the user's to manage (same line as
        // copy/remove draw).
        if preset.trust != dsh_agent_presets::PresetTrust::User {
            let refused = dsh_agent_presets::PresetNotWritableError {
                preset_id: preset.id.clone(),
                reason: "it ships with the deployment".to_string(),
            };
            return self.preset_error(rpc_id, &agent_preset, refused.to_string());
        }
        // The id resolved against the Host's own roots is what selects the
        // directory — no browser payload carries a path unless the
        // deployment has no opener to hand it to.
        let directory = std::path::Path::new(&preset.path)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| preset.path.clone());
        if !self.can_open_paths() {
            return ok(
                rpc_id,
                crate::api::agent_presets::AgentPresetOpenDocumentResult {
                    opened: false,
                    path: Some(directory),
                },
            );
        }
        match self.open_target(rpc_id.clone(), directory, signal).await {
            Ok(()) => ok(
                rpc_id,
                crate::api::agent_presets::AgentPresetOpenDocumentResult {
                    opened: true,
                    path: None,
                },
            ),
            Err(error) => err(rpc_id, error),
        }
    }

    /// `agentPreset.remove`: delete a locally authored preset; shipped
    /// presets are refused (TS `agentPresets.remove`).
    async fn agent_preset_remove(
        &self,
        request: RpcRequest<crate::api::agent_presets::AgentPresetRemoveRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        let agent_preset = request.payload.agent_preset.clone();
        let Some(presets) = self.agent_presets() else {
            return self.no_roster(rpc_id, &agent_preset);
        };
        match presets.remove(&request.payload.agent_preset).await {
            Ok(()) => ok(rpc_id, serde_json::json!({})),
            Err(error) => self.preset_error(rpc_id, &agent_preset, error),
        }
    }
}

/// Map a closed code + path + message into the wire error body.
fn code_rpc_error(code: crate::api::rpc::RpcErrorCode, path: &str, message: &str) -> RpcError {
    let body = RpcErrorBody {
        message: message.to_string(),
        details: crate::api::rpc::PathDetails {
            path: path.to_string(),
        },
    };
    match code {
        crate::api::rpc::RpcErrorCode::DirectoryUnreadable => RpcError::DirectoryUnreadable(body),
        crate::api::rpc::RpcErrorCode::DirectoryExists => RpcError::DirectoryExists(body),
        crate::api::rpc::RpcErrorCode::DirectoryCreateFailed => {
            RpcError::DirectoryCreateFailed(body)
        }
        _ => RpcError::Internal(RpcErrorBody {
            message: message.to_string(),
            details: EmptyDetails {},
        }),
    }
}

/// Success narrow form.
fn ok<T: serde::Serialize>(rpc_id: RpcId, value: T) -> RpcResponse<serde_json::Value> {
    RpcResponse {
        rpc_id,
        result: RpcResult::ok(serde_json::to_value(value).expect("values serialize")),
    }
}

/// Business-error narrow form.
fn err<T>(rpc_id: RpcId, error: RpcError) -> RpcResponse<T> {
    RpcResponse {
        rpc_id,
        result: RpcResult::fail(error),
    }
}

/// The not-yet-wired domain answer (replaced domain by domain).
fn not_wired<T>(rpc_id: RpcId, method: &str) -> RpcResponse<T> {
    err(
        rpc_id,
        RpcError::Internal(RpcErrorBody {
            message: format!("api-proxy: {method} is not implemented in the Rust composition yet"),
            details: EmptyDetails {},
        }),
    )
}

#[async_trait]
impl ApiProxyCarrier for ApiProxyService {
    async fn invoke(
        &self,
        method: &str,
        request: RpcRequest<serde_json::Value>,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let rpc_id = request.rpc_id.clone();
        match method {
            "host.describe" => {
                self.host_describe(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "host.pickDirectory" => {
                self.host_pick_directory(
                    RpcRequest {
                        rpc_id,
                        payload: request.payload,
                    },
                    signal,
                )
                .await
            }
            "host.listDirectory" => {
                let payload: HostListDirectoryRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("host.listDirectory", error)),
                    };
                self.host_list_directory(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "host.createDirectory" => {
                let payload: HostCreateDirectoryRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("host.createDirectory", error));
                        }
                    };
                self.host_create_directory(RpcRequest { rpc_id, payload })
                    .await
            }
            "host.openPath" => {
                let payload: HostOpenPathRequest = match serde_json::from_value(request.payload) {
                    Ok(payload) => payload,
                    Err(error) => return err(rpc_id, bad_request("host.openPath", error)),
                };
                self.host_open_path(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "skill.list" => {
                let payload: crate::api::skills::SkillListRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("skill.list", error)),
                    };
                self.skill_list(RpcRequest { rpc_id, payload }).await
            }
            "credentials.describe" => {
                let payload: crate::api::credentials::CredentialsDescribeRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("credentials.describe", error));
                        }
                    };
                self.credentials_describe(RpcRequest { rpc_id, payload })
                    .await
            }
            "credentials.set" => {
                let payload: crate::api::credentials::CredentialsSetRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("credentials.set", error)),
                    };
                self.credentials_set(RpcRequest { rpc_id, payload }).await
            }
            "credentials.unset" => {
                let payload: crate::api::credentials::CredentialsUnsetRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("credentials.unset", error)),
                    };
                self.credentials_unset(RpcRequest { rpc_id, payload }).await
            }
            "goal.create" => {
                let payload: crate::api::goals::GoalCreateRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.create", error)),
                    };
                self.goal_create(RpcRequest { rpc_id, payload }).await
            }
            "goal.edit" => {
                let payload: crate::api::goals::GoalEditRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.edit", error)),
                    };
                self.goal_edit(RpcRequest { rpc_id, payload }).await
            }
            "goal.pause" => {
                let payload: crate::api::goals::GoalVerbRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.pause", error)),
                    };
                self.goal_verb(RpcRequest { rpc_id, payload }, GoalVerb::Pause)
                    .await
            }
            "goal.resume" => {
                let payload: crate::api::goals::GoalVerbRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.resume", error)),
                    };
                self.goal_verb(RpcRequest { rpc_id, payload }, GoalVerb::Resume)
                    .await
            }
            "goal.complete" => {
                let payload: crate::api::goals::GoalVerbRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.complete", error)),
                    };
                self.goal_verb(RpcRequest { rpc_id, payload }, GoalVerb::Complete)
                    .await
            }
            "goal.clear" => {
                let payload: crate::api::goals::GoalClearRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("goal.clear", error)),
                    };
                self.goal_clear(RpcRequest { rpc_id, payload }).await
            }
            "llm.providers" => {
                self.llm_providers(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "llm.models" => {
                self.llm_models(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "llm.discoverModels" => {
                let payload: crate::api::llm::LlmDiscoverModelsRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("llm.discoverModels", error)),
                    };
                self.llm_discover_models(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "settings.describe" => {
                self.settings_describe(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "settings.update" => {
                let payload: crate::api::settings::SettingsUpdateRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("settings.update", error)),
                    };
                self.settings_write(
                    rpc_id,
                    payload.ns,
                    SettingsWrite::Update {
                        patch: payload.patch,
                        expected_revision: payload.expected_revision.map(|value| value as u64),
                    },
                )
                .await
            }
            "settings.replace" => {
                let payload: crate::api::settings::SettingsReplaceRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("settings.replace", error)),
                    };
                self.settings_write(
                    rpc_id,
                    payload.ns,
                    SettingsWrite::Replace {
                        section: payload.section,
                        expected_revision: payload.expected_revision.map(|value| value as u64),
                    },
                )
                .await
            }
            "settings.mutate" => {
                let payload: crate::api::settings::SettingsMutateRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("settings.mutate", error)),
                    };
                self.settings_write(
                    rpc_id,
                    payload.ns,
                    SettingsWrite::Mutate {
                        ops: payload.ops,
                        expected_revision: payload.expected_revision.map(|value| value as u64),
                    },
                )
                .await
            }
            "settings.openDocument" => {
                self.settings_open_document(
                    RpcRequest {
                        rpc_id,
                        payload: request.payload,
                    },
                    signal,
                )
                .await
            }
            "workspace.list" => {
                self.workspace_list(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "workspace.create" => {
                let payload: crate::api::workspace::WorkspaceCreateRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("workspace.create", error)),
                    };
                self.workspace_create(RpcRequest { rpc_id, payload }).await
            }
            "workspace.rename" => {
                let payload: crate::api::workspace::WorkspaceRenameRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("workspace.rename", error)),
                    };
                self.workspace_rename(RpcRequest { rpc_id, payload }).await
            }
            "workspace.delete" => {
                let payload: crate::api::workspace::WorkspaceDeleteRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("workspace.delete", error)),
                    };
                self.workspace_delete(RpcRequest { rpc_id, payload }).await
            }
            "workspace.insertBefore" => {
                let payload: crate::api::workspace::WorkspaceInsertBeforeRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("workspace.insertBefore", error));
                        }
                    };
                self.workspace_insert_before(RpcRequest { rpc_id, payload })
                    .await
            }
            "workspace.archiveSession" => {
                let payload: crate::api::workspace::WorkspaceArchiveSessionRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("workspace.archiveSession", error));
                        }
                    };
                self.workspace_archive_session(RpcRequest { rpc_id, payload }, false)
                    .await
            }
            "workspace.unarchiveSession" => {
                let payload: crate::api::workspace::WorkspaceArchiveSessionRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("workspace.unarchiveSession", error));
                        }
                    };
                self.workspace_archive_session(RpcRequest { rpc_id, payload }, true)
                    .await
            }
            "session.list" => {
                let payload: crate::api::sessions::SessionListRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.list", error)),
                    };
                self.session_list(RpcRequest { rpc_id, payload }).await
            }
            "session.create" => {
                let payload: crate::api::sessions::SessionCreateRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.create", error)),
                    };
                self.session_create(RpcRequest { rpc_id, payload }).await
            }
            "session.rename" => {
                let payload: crate::api::sessions::SessionRenameRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.rename", error)),
                    };
                self.session_rename(RpcRequest { rpc_id, payload }).await
            }
            "session.cancel" => {
                let payload: crate::api::sessions::SessionRefRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.cancel", error)),
                    };
                self.session_cancel(RpcRequest { rpc_id, payload }).await
            }
            "session.history" => {
                let payload: crate::api::sessions::SessionHistoryRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.history", error)),
                    };
                self.session_history(RpcRequest { rpc_id, payload }).await
            }
            "session.models" => {
                let payload: crate::api::sessions::SessionRefRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.models", error)),
                    };
                self.session_models(RpcRequest { rpc_id, payload }).await
            }
            "session.selectModel" => {
                let payload: crate::api::sessions::SessionSelectModelRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("session.selectModel", error));
                        }
                    };
                self.session_select_model(RpcRequest { rpc_id, payload })
                    .await
            }
            "session.fork" => {
                let payload: crate::api::sessions::SessionForkRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.fork", error)),
                    };
                self.session_fork(RpcRequest { rpc_id, payload }).await
            }
            "session.updateQueue" => {
                let payload: crate::api::sessions::SessionUpdateQueueRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("session.updateQueue", error));
                        }
                    };
                self.session_update_queue(RpcRequest { rpc_id, payload })
                    .await
            }
            "session.prompt" => {
                let payload: crate::api::sessions::SessionPromptRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.prompt", error)),
                    };
                self.session_prompt(RpcRequest { rpc_id, payload }).await
            }
            "session.attachment" => {
                let payload: crate::api::sessions::SessionAttachmentRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.attachment", error)),
                    };
                self.session_attachment(RpcRequest { rpc_id, payload })
                    .await
            }
            "session.search" => {
                let payload: crate::api::sessions::SessionSearchRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("session.search", error)),
                    };
                self.session_search(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "subagent.list" => {
                let payload: crate::api::subagents::SubagentListRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("subagent.list", error)),
                    };
                self.subagent_list(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "subagent.history" => {
                let payload: crate::api::subagents::SubagentHistoryRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("subagent.history", error)),
                    };
                self.subagent_history(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "subagent.prompt" => {
                let payload: crate::api::subagents::SubagentPromptRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("subagent.prompt", error)),
                    };
                self.subagent_prompt(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "subagent.interrupt" => {
                let payload: crate::api::subagents::SubagentInterruptRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("subagent.interrupt", error)),
                    };
                self.subagent_interrupt(RpcRequest { rpc_id, payload })
                    .await
            }
            "agentPreset.list" => {
                self.agent_preset_list(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "agentPreset.select" => {
                let payload: crate::api::agent_presets::AgentPresetSelectRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("agentPreset.select", error)),
                    };
                self.agent_preset_select(RpcRequest { rpc_id, payload })
                    .await
            }
            "agentPreset.read" => {
                let payload: crate::api::agent_presets::AgentPresetReadRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("agentPreset.read", error)),
                    };
                self.agent_preset_read(RpcRequest { rpc_id, payload }).await
            }
            "agentPreset.copy" => {
                let payload: crate::api::agent_presets::AgentPresetCopyRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("agentPreset.copy", error)),
                    };
                self.agent_preset_copy(RpcRequest { rpc_id, payload }).await
            }
            "agentPreset.openDocument" => {
                let payload: crate::api::agent_presets::AgentPresetOpenDocumentRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("agentPreset.openDocument", error));
                        }
                    };
                self.agent_preset_open_document(RpcRequest { rpc_id, payload }, signal)
                    .await
            }
            "agentPreset.remove" => {
                let payload: crate::api::agent_presets::AgentPresetRemoveRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => return err(rpc_id, bad_request("agentPreset.remove", error)),
                    };
                self.agent_preset_remove(RpcRequest { rpc_id, payload })
                    .await
            }
            other => not_wired(rpc_id, other),
        }
    }

    /// The mux event channel: a subscribed baseline per attached session,
    /// then live `session/event` frames. Approval/question/jobs/projection
    /// baselines arrive with their owning milestones (deviation: the TS
    /// stream also replays pending approvals/questions and queue/jobs
    /// snapshots on open).
    fn events_mux(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = FrameRequest> + Send>> {
        use futures::StreamExt;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<FrameRequest>();
        // Baseline: one subscribed control frame per attached session.
        for session in self
            .sessions()
            .map(|store| store.list())
            .unwrap_or_default()
        {
            let _ = tx.send(FrameRequest {
                rpc_id: crate::api::rpc::rpc_id(Self::fresh_id()),
                payload: serde_json::json!({
                    "type": "session/subscribed",
                    "sessionId": session.id(),
                    "lastSeq": session.seq() as i64 - 1,
                }),
            });
        }
        // Register after the session baseline; subscribe() atomically inserts
        // the queue and replays every still-pending interaction, so a request
        // created during baseline construction is retained rather than lost.
        let subscription = self.interactions.subscribe(tx.clone());
        // Live session events ride the global cordis stream.
        let tx_for_listener = tx.clone();
        let listener: Arc<cordis::Listener> = Arc::new(
            move |_dispatch_ctx: &Context, args: Vec<cordis::ArcValue>| {
                let tx = tx_for_listener.clone();
                Box::pin(async move {
                    let session = args
                        .first()
                        .and_then(|value| cordis::downcast::<dsh_session::Session>(value))
                        .cloned();
                    let event = args
                        .get(1)
                        .and_then(|value| cordis::downcast::<dsh_session::SessionEvent>(value))
                        .cloned();
                    if let (Some(session), Some(event)) = (session, event) {
                        let _ = tx.send(FrameRequest {
                            rpc_id: crate::api::rpc::rpc_id(Self::fresh_id()),
                            payload: serde_json::json!({
                                "type": "session/event",
                                "sessionId": session.id(),
                                "event": event,
                            }),
                        });
                    }
                    None
                })
            },
        );
        let listener_disposer = self.ctx.events.register(
            &self.ctx,
            "api-proxy: mux session events",
            "session/event",
            listener,
            &cordis::EventOptions::default().global(true),
        );
        // `register` normally anchors cleanup to the root fiber. This listener
        // belongs to one connection, so transfer sole ownership to the stream.
        self.ctx.fiber.disposables.delete(&listener_disposer);
        let resources = crate::interactions::MuxResources::new(subscription, listener_disposer);
        // The open comment rides the carrier's SSE framing; the stream
        // itself yields frames until the signal aborts.
        let stream_signal = signal.clone();
        let stream = futures::stream::unfold((rx, resources), move |(mut rx, resources)| {
            let signal = stream_signal.clone();
            async move {
                tokio::select! {
                    biased;
                    _ = signal.cancelled() => None,
                    frame = rx.recv() => frame.map(|frame| (frame, (rx, resources))),
                }
            }
        });
        let _ = request;
        Box::pin(stream)
    }

    fn events_host(
        &self,
        request: FrameRequest,
        signal: AbortSignal,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = FrameRequest> + Send>> {
        use futures::StreamExt;

        /// The host events this application forwards to consumers verbatim
        /// (TS `API_REMOTE_FORWARDED_EVENTS`): no projection, no redaction,
        /// no renaming.
        const REMOTE_FORWARDED: [&str; 11] = [
            "agent-preset/selected",
            "commands/change",
            "credentials/updated",
            "cordis/request-run",
            "cordis/request-run-resolved",
            "cordis/dynamic-package",
            "cordis/dynamic-retract",
            "cordis/inspect-query",
            "cordis/inspect-query-resolved",
            "llm/adapters-updated",
            "settings/document-updated",
        ];

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<FrameRequest>();
        let ctx = self.ctx.clone();
        let workspace_registry = self.workspace_registry();
        let committed = workspace_registry
            .as_ref()
            .and_then(|registry| registry.list().ok())
            .unwrap_or_default();

        // Frame-dedup baselines, same posture as TS: the stream opens
        // against the current set; workspace.list re-baselines reconnecting
        // clients, so only later changes need frames.
        let committed_ids: Arc<parking_lot::Mutex<std::collections::HashSet<String>>> =
            Arc::new(parking_lot::Mutex::new(
                committed
                    .iter()
                    .map(|workspace| workspace.id().to_string())
                    .collect(),
            ));
        let committed_order: Arc<parking_lot::Mutex<Vec<dsh_workspace::WorkspaceId>>> =
            Arc::new(parking_lot::Mutex::new(
                committed
                    .iter()
                    .map(|workspace| workspace.id().clone())
                    .collect(),
            ));
        let archived_ids: Arc<parking_lot::Mutex<Vec<dsh_session::SessionId>>> =
            Arc::new(parking_lot::Mutex::new(
                workspace_registry
                    .as_ref()
                    .map(|registry| registry.archived_session_ids())
                    .unwrap_or_default(),
            ));

        /// Push one host frame onto the stream.
        fn push(
            tx: &tokio::sync::mpsc::UnboundedSender<FrameRequest>,
            frame: crate::api::events::HostFrame,
        ) {
            let _ = tx.send(FrameRequest {
                rpc_id: crate::api::rpc::rpc_id(ApiProxyService::fresh_id()),
                payload: serde_json::to_value(&frame).unwrap_or(serde_json::Value::Null),
            });
        }

        tokio::spawn(async move {
            // session/created → host/session-added.
            let tx_created = tx.clone();
            let _d_created = ctx
                .on(
                    "session/created",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_created.clone();
                        Box::pin(async move {
                            if let Some(session) = args
                                .first()
                                .and_then(|value| cordis::downcast::<dsh_session::Session>(value))
                                .cloned()
                            {
                                let header = session.header();
                                let blank = !session
                                    .events()
                                    .iter()
                                    .any(|event| event.type_ == "turn/start");
                                push(
                                    &tx,
                                    crate::api::events::HostFrame::SessionAdded {
                                        session_id: session.id().clone(),
                                        blank,
                                        parent_session_id: header.parent_session.clone(),
                                        origin: header.origin.as_deref().and_then(|origin| {
                                            match origin {
                                                "subagent" => Some(
                                                    crate::api::events::HostSessionOrigin::Subagent,
                                                ),
                                                _ => None,
                                            }
                                        }),
                                        cwd: header.cwd.clone(),
                                        agent_preset: header.agent_preset.clone(),
                                    },
                                );
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            // session/disposed → host/session-removed.
            let tx_disposed = tx.clone();
            let _d_disposed = ctx
                .on(
                    "session/disposed",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_disposed.clone();
                        Box::pin(async move {
                            if let Some(session) = args
                                .first()
                                .and_then(|value| cordis::downcast::<dsh_session::Session>(value))
                                .cloned()
                            {
                                push(
                                    &tx,
                                    crate::api::events::HostFrame::SessionRemoved {
                                        session_id: session.id().clone(),
                                    },
                                );
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            // workspace/session-deleted → host/session-removed.
            let tx_session_deleted = tx.clone();
            let _d_session_deleted = ctx
                .on(
                    "workspace/session-deleted",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_session_deleted.clone();
                        Box::pin(async move {
                            if let Some(session_id) = args
                                .first()
                                .and_then(|value| cordis::downcast::<dsh_session::SessionId>(value))
                                .cloned()
                            {
                                push(
                                    &tx,
                                    crate::api::events::HostFrame::SessionRemoved { session_id },
                                );
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            // domain/changed → the workspace frame family. Deviations: the
            // agent/status and agent/error frames wait for dsh-agent's
            // status/error publication (the Rust registry only publishes
            // agent/created + agent/disposed so far); a committed
            // workspace id the registry cannot resolve is skipped instead
            // of throwing (the Rust listener has no throw path).
            let tx_domain = tx.clone();
            let domain_ids = committed_ids.clone();
            let domain_order = committed_order.clone();
            let domain_archived = archived_ids.clone();
            let domain_registry = workspace_registry.clone();
            let _d_domain = ctx
                .on(
                    "domain/changed",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_domain.clone();
                        let committed_ids = domain_ids.clone();
                        let committed_order = domain_order.clone();
                        let archived_ids = domain_archived.clone();
                        let registry = domain_registry.clone();
                        Box::pin(async move {
                            let Some(change) = args
                                .first()
                                .and_then(|value| {
                                    cordis::downcast::<dsh_storage_domain::DomainChanged>(value)
                                })
                                .cloned()
                            else {
                                return None;
                            };
                            match change {
                                dsh_storage_domain::DomainChanged::Put {
                                    domain,
                                    table,
                                    value,
                                    ..
                                } if domain == "workspace" && table.is_empty() => {
                                    let Ok(state) = serde_json::from_value::<
                                        dsh_workspace::spec::WorkspaceDomainState,
                                    >(value) else {
                                        return None;
                                    };
                                    let mut ids = committed_ids.lock();
                                    let order_changed = {
                                        let order = committed_order.lock();
                                        state.workspace_ids.len() == order.len()
                                            && state
                                                .workspace_ids
                                                .iter()
                                                .all(|id| ids.contains(&id.to_string()))
                                            && state
                                                .workspace_ids
                                                .iter()
                                                .enumerate()
                                                .any(|(index, id)| *id != order[index])
                                    };
                                    for workspace_id in &state.workspace_ids {
                                        if ids.contains(&workspace_id.to_string()) {
                                            continue;
                                        }
                                        let Some(registry) = registry.as_ref() else {
                                            continue;
                                        };
                                        let Some(workspace) = registry.get(workspace_id) else {
                                            continue;
                                        };
                                        ids.insert(workspace_id.to_string());
                                        push(
                                            &tx,
                                            crate::api::events::HostFrame::WorkspaceChanged {
                                                workspace: Self::workspace_view(&workspace),
                                            },
                                        );
                                    }
                                    drop(ids);
                                    *committed_order.lock() = state.workspace_ids.clone();
                                    if order_changed {
                                        push(
                                            &tx,
                                            crate::api::events::HostFrame::WorkspaceOrderChanged {
                                                workspace_ids: state
                                                    .workspace_ids
                                                    .iter()
                                                    .map(|id| {
                                                        crate::api::workspace::WorkspaceId::new(
                                                            id.to_string(),
                                                        )
                                                    })
                                                    .collect(),
                                            },
                                        );
                                    }
                                    let mut archived = archived_ids.lock();
                                    if state.archived_session_ids != *archived {
                                        *archived = state.archived_session_ids.clone();
                                        push(
                                        &tx,
                                        crate::api::events::HostFrame::ArchivedSessionsChanged {
                                            archived_session_ids: state.archived_session_ids,
                                        },
                                    );
                                    }
                                }
                                dsh_storage_domain::DomainChanged::Deleted {
                                    domain,
                                    table,
                                    key,
                                } if domain == "workspace" && table == "workspaces" => {
                                    if !committed_ids.lock().remove(&key) {
                                        return None;
                                    }
                                    push(
                                        &tx,
                                        crate::api::events::HostFrame::WorkspaceRemoved {
                                            workspace_id: crate::api::workspace::WorkspaceId::new(
                                                key,
                                            ),
                                        },
                                    );
                                }
                                dsh_storage_domain::DomainChanged::Put {
                                    domain,
                                    table,
                                    key,
                                    value,
                                } if domain == "workspace" && table == "workspaces" => {
                                    if !committed_ids.lock().contains(&key) {
                                        return None;
                                    }
                                    // Existing-entity table writes are complete
                                    // attach/touch commits; a new entity's first
                                    // put waits for the global registry write.
                                    if let Some(view) = Self::workspace_record_view(&key, &value) {
                                        push(
                                            &tx,
                                            crate::api::events::HostFrame::WorkspaceChanged {
                                                workspace: view,
                                            },
                                        );
                                    }
                                }
                                _ => {}
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            // Allowlisted host events ride one verbatim wrapper frame each.
            for name in REMOTE_FORWARDED {
                let tx_remote = tx.clone();
                let _d_remote = ctx
                    .on(
                        name,
                        Arc::new(move |_dispatch_ctx, args| {
                            let tx = tx_remote.clone();
                            let name = name.to_string();
                            Box::pin(async move {
                                // Only JSON-serializable arguments are forwarded
                                // (TS `assertJsonArgs`; the Rust side skips
                                // non-JSON args instead of throwing).
                                let json_args: Vec<serde_json::Value> = args
                                    .iter()
                                    .filter_map(|value| {
                                        if let Some(json) =
                                            cordis::downcast::<serde_json::Value>(value)
                                        {
                                            return Some(json.clone());
                                        }
                                        if let Some(text) = cordis::downcast::<String>(value) {
                                            return Some(serde_json::Value::String(text.clone()));
                                        }
                                        None
                                    })
                                    .collect();
                                push(
                                    &tx,
                                    crate::api::events::HostFrame::RemoteEvent {
                                        event: name,
                                        args: json_args,
                                    },
                                );
                                None
                            })
                        }),
                        cordis::EventOptions::default().global(true),
                    )
                    .await;
            }
            // Hold the listeners for the stream's lifetime (the spawned task
            // outlives the stream; disposers release on process teardown,
            // same retention posture as the mux stream).
            std::future::pending::<()>().await;
        });
        let _ = request;
        let stream_signal = signal.clone();
        let stream = futures::stream::unfold(rx, move |mut rx| {
            let signal = stream_signal.clone();
            async move {
                loop {
                    if signal.aborted() {
                        return None;
                    }
                    match rx.recv().await {
                        Some(frame) => return Some((frame, rx)),
                        None => return None,
                    }
                }
            }
        });
        Box::pin(stream)
    }

    async fn respond(&self, response: ClientResponse) -> crate::api::rpc::RpcReceipt {
        self.interactions.respond(response)
    }

    async fn session_log(&self, query: SessionLogQuery, signal: AbortSignal) -> DownloadResponse {
        use crate::session_export::{
            SessionLogExportDeps, assemble_session_log_zip, flush_live_session_log,
            session_log_zip_entries, session_log_zip_filename,
        };

        // Clean error path first: missing services answer 500 and a missing
        // root artifact 404 before any zip byte is produced.
        let deps = SessionLogExportDeps {
            session_query: self
                .ctx
                .get_typed::<Arc<dsh_session_query::SessionQueryEngine>>("sessionQuery", false)
                .map(|slot| slot.as_ref().clone()),
            session_persistence: self
                .ctx
                .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                    "sessionPersistence",
                    false,
                )
                .map(|slot| slot.as_ref().clone()),
            attachments: self
                .ctx
                .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
                .map(|slot| slot.as_ref().clone()),
            sessions: self.sessions(),
        };
        if deps.session_query.is_none()
            || deps.session_persistence.is_none()
            || deps.attachments.is_none()
        {
            return DownloadResponse {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                headers: Vec::new(),
                body: Some(
                    b"session log export is unavailable: missing session-query, session-persistence, or attachments service"
                        .to_vec(),
                ),
            };
        }
        let persistence = deps.session_persistence.as_ref().expect("checked");
        if !persistence.supports_raw_artifacts() {
            return DownloadResponse {
                status: http::StatusCode::NOT_IMPLEMENTED,
                headers: Vec::new(),
                body: Some(
                    b"session log export is unavailable: the persistence backend does not expose per-session raw artifacts"
                        .to_vec(),
                ),
            };
        }
        let session_id = dsh_session::session_id(query.session_id.clone());
        if flush_live_session_log(&deps, &session_id, &signal)
            .await
            .is_err()
            || signal.aborted()
        {
            return DownloadResponse {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                headers: Vec::new(),
                body: Some(b"session log export failed to prepare the stored artifact".to_vec()),
            };
        }
        let root = match persistence.read_raw(&session_id).await {
            Ok(root) => root,
            Err(_) => {
                return DownloadResponse {
                    status: http::StatusCode::INTERNAL_SERVER_ERROR,
                    headers: Vec::new(),
                    body: Some(
                        b"session log export failed to prepare the stored artifact".to_vec(),
                    ),
                };
            }
        };
        let Some(root) = root else {
            return DownloadResponse {
                status: http::StatusCode::NOT_FOUND,
                headers: Vec::new(),
                body: Some(b"session not found".to_vec()),
            };
        };
        let entries = match session_log_zip_entries(
            &deps,
            &root,
            &session_id,
            query.include_descendants.unwrap_or(false),
            &signal,
        )
        .await
        {
            Ok(entries) => entries,
            Err(_) => {
                return DownloadResponse {
                    status: http::StatusCode::INTERNAL_SERVER_ERROR,
                    headers: Vec::new(),
                    body: Some(b"session log export failed".to_vec()),
                };
            }
        };
        let bytes = match assemble_session_log_zip(
            entries,
            self.defaults.session_export_compression_level.min(9) as u8,
        ) {
            Ok(bytes) => bytes,
            Err(_) => {
                return DownloadResponse {
                    status: http::StatusCode::INTERNAL_SERVER_ERROR,
                    headers: Vec::new(),
                    body: Some(b"session log export failed".to_vec()),
                };
            }
        };
        DownloadResponse {
            status: http::StatusCode::OK,
            headers: vec![
                ("content-type".to_string(), "application/zip".to_string()),
                (
                    "content-disposition".to_string(),
                    format!(
                        "attachment; filename=\"{}\"",
                        session_log_zip_filename(&session_id)
                    ),
                ),
            ],
            body: Some(bytes),
        }
    }
}

/// `bad-request` for a payload that failed its second parse.
fn bad_request(method: &str, error: serde_json::Error) -> RpcError {
    RpcError::BadRequest(RpcErrorBody {
        message: format!("invalid payload for {method}"),
        details: crate::api::rpc::BadRequestDetails {
            issues: vec![serde_json::json!({ "error": error.to_string() })],
        },
    })
}

#[allow(unused)]
fn _vocab_anchors() {
    // Keep the carrier types referenced while the wiring grows.
    let _ = Body::Bytes(Vec::new());
}
