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
use cordis::{Context, make_disposer};
use dsh_agent::{Agent, AgentSetup, ModelSelectionRef};
use dsh_host_directory_picker::{
    AbortSignal as PickerAbort, DirectoryPicker, DirectoryPickerBrowseCapability,
    DirectoryPickerCapability, DirectoryPickerErrorCode, DirectoryPickerListError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;

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

const HISTORY_SCAN_EVENT_LIMIT: usize = 65_536;
const HISTORY_SOURCE_BYTE_LIMIT: usize = 64 * 1024 * 1024;
const HISTORY_TRANSPORT_EVENT_LIMIT: usize = 4_096;
const HISTORY_TRANSPORT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// Keep unreadable histories distinct from genuinely absent sessions.
fn history_read_error(session_id: &str, message: String) -> RpcError {
    if message == format!("session \"{session_id}\" not found") {
        RpcError::SessionNotFound(RpcErrorBody {
            message,
            details: crate::api::rpc::SessionIdDetails {
                session_id: session_id.to_string(),
            },
        })
    } else {
        RpcError::Internal(RpcErrorBody {
            message: format!("session.history: {message}"),
            details: EmptyDetails {},
        })
    }
}

#[cfg(test)]
mod history_error_tests {
    #[test]
    fn corrupt_or_oversized_history_is_not_reported_as_missing() {
        let id = "agent-session-test";
        assert!(matches!(
            super::history_read_error(id, format!("session \"{id}\" not found")),
            crate::api::rpc::RpcError::SessionNotFound(_)
        ));
        let error =
            super::history_read_error(id, "one safe history group requires 7305 events".into());
        assert!(matches!(error, crate::api::rpc::RpcError::Internal(_)));
    }
}

type OpenPathFn =
    Arc<dyn Fn(String, AbortSignal) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
type GoalMutation = Arc<
    dyn Fn(
            Arc<dsh_goal::GoalService>,
            Arc<dyn Agent>,
        ) -> Result<dsh_goal::GoalView, dsh_goal::GoalError>
        + Send
        + Sync,
>;
type PresetSwitch = (
    u64,
    futures::future::Shared<BoxFuture<'static, Arc<RpcResponse<serde_json::Value>>>>,
);
type PresetSwitches =
    Arc<parking_lot::Mutex<std::collections::HashMap<dsh_session::SessionId, PresetSwitch>>>;

#[derive(Debug)]
enum SubagentPromptPart {
    Text(String),
    Image(usize),
}

fn subagent_attachment_error(
    rpc_id: RpcId,
    message: impl Into<String>,
    reason: impl Into<String>,
) -> RpcResponse<serde_json::Value> {
    err(
        rpc_id,
        RpcError::AttachmentError(RpcErrorBody {
            message: message.into(),
            details: crate::api::rpc::ReasonDetails {
                reason: reason.into(),
            },
        }),
    )
}

/// The host app version reported by `host.describe` (the TS placeholder —
/// reads apps/cli's package version once the CLI lands).
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Composition inputs supplied by the host app (TS `ApiProxyDefaults`).
pub struct ApiProxyDefaults {
    /// The model selection a session starts from when its own log names
    /// none. Read on every access rather than captured.
    pub default_model_selection: Arc<dyn Fn() -> ModelSelection + Send + Sync>,
    /// Default project directory for new sessions whose create request
    /// carries no cwd.
    pub cwd: String,
    /// Resolved Harness data home reported to the browser generation.
    pub dsh_home: String,
    /// Native open-with-default-application; injectable for carrier tests.
    pub open_path: Option<OpenPathFn>,
    /// Whether handing a path to the native opener can work at all.
    pub can_open_path: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Profile-local plugin entry document used by inventory mutations.
    pub plugins_document: Option<std::path::PathBuf>,
    /// Serialize Loader mutation, whole-tree snapshot, persistence, and rollback.
    pub plugin_mutation_lock: Arc<tokio::sync::Mutex<()>>,
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
            dsh_home: dsh_home_paths::resolve_dsh_home(None, &|key| std::env::var(key).ok())
                .to_string_lossy()
                .into_owned(),
            open_path: None,
            can_open_path: None,
            plugins_document: None,
            plugin_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            session_export_compression_level: 6,
            cold_blank_probe_max_bytes: 1024,
        }
    }
}

type SelectionState = Arc<Mutex<ModelSelectionRef>>;

struct SelectionEntry {
    agent: std::sync::Weak<dyn Agent>,
    state: SelectionState,
    installed: tokio::sync::OnceCell<()>,
}

type SelectionMap =
    Arc<Mutex<std::collections::HashMap<dsh_session::SessionId, Arc<SelectionEntry>>>>;

fn core_selection(selection: crate::api::sessions::ModelSelection) -> dsh_agent::ModelSelection {
    dsh_agent::ModelSelection {
        provider: selection.provider,
        model: selection.model,
        reasoning_effort: selection
            .reasoning_effort
            .map(dsh_llm::ReasoningEffortId::new),
    }
}

fn wire_selection(selection: dsh_agent::ModelSelection) -> crate::api::sessions::ModelSelection {
    crate::api::sessions::ModelSelection {
        provider: selection.provider,
        model: selection.model,
        reasoning_effort: selection.reasoning_effort.map(|effort| effort.to_string()),
    }
}

fn model_selection_from_events(
    events: &[dsh_session::SessionEvent],
) -> Option<dsh_agent::ModelSelection> {
    if let Some(selected) = events.iter().rev().find_map(|event| {
        if event.type_ != "model/selection" {
            return None;
        }
        serde_json::from_value::<crate::api::sessions::ModelSelection>(event.data.clone())
            .ok()
            .map(core_selection)
    }) {
        return Some(selected);
    }
    events.iter().rev().find_map(|event| {
        if event.type_ != "request/header" {
            return None;
        }
        let config = event.data.get("header")?.get("config")?;
        Some(dsh_agent::ModelSelection {
            provider: config.get("provider")?.as_str()?.to_string(),
            model: config.get("model")?.as_str()?.to_string(),
            reasoning_effort: config
                .get("reasoningEffort")
                .and_then(serde_json::Value::as_str)
                .map(dsh_llm::ReasoningEffortId::new),
        })
    })
}

fn persisted_model_selection(session: &dsh_session::Session) -> Option<dsh_agent::ModelSelection> {
    model_selection_from_events(&session.events())
}

fn set_plugin_document_enabled(
    entries: &mut [serde_json::Value],
    entry_id: &str,
    enabled: bool,
) -> bool {
    for entry in entries {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        if object.get("id").and_then(serde_json::Value::as_str) == Some(entry_id) {
            object.insert("disabled".to_string(), serde_json::Value::Bool(!enabled));
            return true;
        }
        if let Some(children) = object
            .get_mut("config")
            .and_then(serde_json::Value::as_array_mut)
            && set_plugin_document_enabled(children, entry_id, enabled)
        {
            return true;
        }
    }
    false
}

fn model_selection_setup(defaults: Arc<ApiProxyDefaults>, selections: SelectionMap) -> AgentSetup {
    Arc::new(move |agent_ctx, agent| {
        let ctx = agent_ctx.clone();
        let id = agent.id().clone();
        let weak = Arc::downgrade(&agent);
        let defaults = defaults.clone();
        let selections = selections.clone();
        Box::pin(async move {
            let entry = {
                let mut entries = selections.lock();
                if let Some(existing) = entries.get(&id) {
                    if let Some(existing_agent) = existing.agent.upgrade() {
                        if !Arc::ptr_eq(&existing_agent, &agent) {
                            return Err(format!(
                                "api-proxy: model selection already belongs to another live agent \"{id}\""
                            ));
                        }
                        existing.clone()
                    } else {
                        entries.remove(&id);
                        let session = agent.session().clone();
                        let defaults = defaults.clone();
                        let entry = Arc::new(SelectionEntry {
                            agent: weak.clone(),
                            state: Arc::new(Mutex::new(ModelSelectionRef::with_resolver(
                                Arc::new(move || {
                                    if let Some(selected) = persisted_model_selection(&session) {
                                        return Some(selected);
                                    }
                                    if let Some(header) = session.request_header() {
                                        return Some(dsh_agent::ModelSelection {
                                            provider: header.config.provider,
                                            model: header.config.model,
                                            reasoning_effort: header.config.reasoning_effort,
                                        });
                                    }
                                    Some(core_selection((defaults.default_model_selection)()))
                                }),
                            ))),
                            installed: tokio::sync::OnceCell::new(),
                        });
                        entries.insert(id.clone(), entry.clone());
                        entry
                    }
                } else {
                    let session = agent.session().clone();
                    let defaults = defaults.clone();
                    let entry = Arc::new(SelectionEntry {
                        agent: weak.clone(),
                        state: Arc::new(Mutex::new(ModelSelectionRef::with_resolver(Arc::new(
                            move || {
                                if let Some(selected) = persisted_model_selection(&session) {
                                    return Some(selected);
                                }
                                if let Some(header) = session.request_header() {
                                    return Some(dsh_agent::ModelSelection {
                                        provider: header.config.provider,
                                        model: header.config.model,
                                        reasoning_effort: header.config.reasoning_effort,
                                    });
                                }
                                Some(core_selection((defaults.default_model_selection)()))
                            },
                        )))),
                        installed: tokio::sync::OnceCell::new(),
                    });
                    entries.insert(id.clone(), entry.clone());
                    entry
                }
            };

            let entry_for_install = entry.clone();
            let entries_for_cleanup = selections.clone();
            let id_for_cleanup = id.clone();
            entry
                .installed
                .get_or_init(|| async move {
                    let _ =
                        dsh_agent::install_model_selection(&ctx, entry_for_install.state.clone())
                            .await;
                    let cleanup_entry = entry_for_install.clone();
                    let _ = ctx.effect(
                        "apiProxy.modelSelection",
                        Box::pin(async move {
                            Some(make_disposer(move || {
                                let entries = entries_for_cleanup.clone();
                                let id = id_for_cleanup.clone();
                                let entry = cleanup_entry.clone();
                                Box::pin(async move {
                                    let mut entries = entries.lock();
                                    if entries
                                        .get(&id)
                                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                                    {
                                        entries.remove(&id);
                                    }
                                })
                            }))
                        }),
                    );
                })
                .await;
            Ok(None)
        })
    })
}

fn composed_agent_setup(
    selection: AgentSetup,
    presets: Option<Arc<dsh_agent_presets::AgentPresets>>,
    preset_id: Option<String>,
) -> AgentSetup {
    Arc::new(move |agent_ctx, agent| {
        let agent_ctx = agent_ctx.clone();
        let selection = selection.clone();
        let presets = presets.clone();
        let preset_id = preset_id.clone();
        Box::pin(async move {
            let commit = selection(&agent_ctx, agent).await?;
            if let Some(presets) = presets {
                presets
                    .mount(&agent_ctx, preset_id.as_deref())
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(commit)
        })
    })
}

struct OwnedAgentHandle {
    agent: Arc<dyn Agent>,
    dispose: Option<BoxFuture<'static, ()>>,
}

type OwnedAgentHandles =
    Arc<parking_lot::Mutex<std::collections::HashMap<dsh_session::SessionId, OwnedAgentHandle>>>;

async fn retire_idle_agent(
    resolver: &Arc<crate::agent_lookup::AgentResolver>,
    handles: &OwnedAgentHandles,
    sessions: Option<Arc<dsh_session::SessionStore>>,
    agents: Option<Arc<dsh_agent::AgentRegistry>>,
    agent: Arc<dyn Agent>,
) {
    let session_id = agent.id().clone();
    let _retirement = resolver.begin_retirement(&session_id);
    if agent.status() != dsh_agent::AgentStatus::Idle || agent.inbox().has_pending() {
        return;
    }
    let Some(agents) = agents else {
        return;
    };
    if agents
        .get(&session_id)
        .is_none_or(|current| !Arc::ptr_eq(&current, &agent))
        || agents
            .list()
            .iter()
            .any(|child| agents.is_owned_by(child.id(), &agent))
    {
        return;
    }
    let Some(sessions) = sessions else {
        return;
    };
    if sessions.flush(agent.session()).await.is_err() {
        return;
    }
    let dispose = {
        let mut handles = handles.lock();
        let exact = handles
            .get(&session_id)
            .is_some_and(|owned| Arc::ptr_eq(&owned.agent, &agent));
        if !exact {
            return;
        }
        handles
            .remove(&session_id)
            .and_then(|mut owned| owned.dispose.take())
    };
    if let Some(dispose) = dispose {
        dispose.await;
    }
}

#[cfg(test)]
mod idle_retirement_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    use dsh_agent::{
        AgentCancelCause, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxNotifications,
        InboxTarget,
    };
    use dsh_scope::ScopeKey;
    use dsh_session::{Session, SessionId, UserMessage, session_id};

    struct StatusAgent {
        id: SessionId,
        options: AgentOptions,
        session: Session,
        inbox: Inbox,
        ctx: Context,
        scope_key: ScopeKey,
        running: AtomicBool,
    }

    impl Agent for StatusAgent {
        fn id(&self) -> &SessionId {
            &self.id
        }

        fn options(&self) -> &AgentOptions {
            &self.options
        }

        fn session(&self) -> &Session {
            &self.session
        }

        fn inbox(&self) -> &Inbox {
            &self.inbox
        }

        fn status(&self) -> AgentStatus {
            if self.running.load(Ordering::SeqCst) {
                AgentStatus::Running
            } else {
                AgentStatus::Idle
            }
        }

        fn ctx(&self) -> &Context {
            &self.ctx
        }

        fn scope_key(&self) -> &ScopeKey {
            &self.scope_key
        }

        fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

        fn when_idle(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }

        fn run_maintenance(
            &self,
            _task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
        ) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }

        fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

        fn followup(&self, _message: UserMessage) {}

        fn steer(&self, _message: UserMessage) {}

        fn inject(&self, _message: UserMessage) {}
    }

    #[tokio::test]
    async fn stale_idle_observation_does_not_retire_a_running_agent() {
        let ctx = Context::root();
        let sessions = dsh_session::SessionStore::install(&ctx);
        let agents = dsh_agent::AgentRegistry::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let id = session_id("stale-idle-retirement");
        let session = sessions
            .create(&ctx, Some(id.clone()), None)
            .await
            .expect("session");
        let inbox = Inbox::new(&session, InboxNotifications::default()).expect("inbox");
        let concrete = Arc::new(StatusAgent {
            id,
            options: AgentOptions::default(),
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            running: AtomicBool::new(true),
        });
        let agent: Arc<dyn Agent> = concrete.clone();
        let detach = agents.enter(agent.clone(), None).expect("enter agent");
        agents.announce(&agent).await.expect("announce agent");

        let disposed = Arc::new(AtomicBool::new(false));
        let disposed_for_future = Arc::clone(&disposed);
        service.retain_owned_handle(dsh_agent::AgentHandle {
            agent: agent.clone(),
            dispose: Box::pin(async move {
                disposed_for_future.store(true, Ordering::SeqCst);
            }),
        });

        service.retire_idle_agent_for_test(agent.clone()).await;

        assert!(
            !disposed.load(Ordering::SeqCst),
            "a stale idle observation must not dispose an Agent that is already running again"
        );
        assert!(
            service.owned_agent_handles.lock().contains_key(agent.id()),
            "the owner handle must remain available for the running lifecycle"
        );

        detach().await;
    }

    #[tokio::test]
    async fn truly_idle_agent_releases_its_owner_handle() {
        let ctx = Context::root();
        let sessions = dsh_session::SessionStore::install(&ctx);
        let agents = dsh_agent::AgentRegistry::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let id = session_id("idle-agent-retirement");
        let session = sessions
            .create(&ctx, Some(id.clone()), None)
            .await
            .expect("session");
        let inbox = Inbox::new(&session, InboxNotifications::default()).expect("inbox");
        let concrete = Arc::new(StatusAgent {
            id,
            options: AgentOptions::default(),
            session,
            inbox,
            ctx: ctx.clone(),
            scope_key: ScopeKey::new(),
            running: AtomicBool::new(false),
        });
        let agent: Arc<dyn Agent> = concrete.clone();
        let detach = agents.enter(agent.clone(), None).expect("enter agent");
        agents.announce(&agent).await.expect("announce agent");

        let disposed = Arc::new(AtomicBool::new(false));
        let disposed_for_future = Arc::clone(&disposed);
        service.retain_owned_handle(dsh_agent::AgentHandle {
            agent: agent.clone(),
            dispose: Box::pin(async move {
                disposed_for_future.store(true, Ordering::SeqCst);
            }),
        });

        service.retire_idle_agent_for_test(agent.clone()).await;

        assert!(disposed.load(Ordering::SeqCst));
        assert!(!service.owned_agent_handles.lock().contains_key(agent.id()));

        detach().await;
    }
}

/// The composed `ctx.apiProxy` service.
pub struct ApiProxyService {
    ctx: Context,
    defaults: Arc<ApiProxyDefaults>,
    resolver: Arc<crate::agent_lookup::AgentResolver>,
    model_selection_setup: AgentSetup,
    /// Per-session process-local model selections, tied to the exact live
    /// Agent identity. The state is installed before publication and removed
    /// by the Agent scope's disposer.
    selections: SelectionMap,
    /// Owner-only lifecycle handles for Agents created or resumed through this API.
    owned_agent_handles: OwnedAgentHandles,
    /// Per-session preset-switch chains (the TS `presetSwitches` map): each
    /// select request serializes behind the previous one so a queued request
    /// re-reads blankness and the roster after earlier switches committed.
    /// The `u64` is a per-session turn token; the settled entry is removed
    /// only when it is still the caller's own turn (TS finally-check).
    preset_switches: PresetSwitches,
    /// Monotone turn tokens for the preset-switch chains.
    preset_switch_counter: std::sync::atomic::AtomicU64,
    /// Pending approval/question requests and live mux subscribers.
    interactions: Arc<crate::interactions::InteractionState>,
    /// History pages can each contain thousands of JSON events. Keep their
    /// decode/view/serialization lifetimes from overlapping when the browser
    /// concurrently opens, gap-repairs, and jumps through one conversation.
    history_gate: Arc<tokio::sync::Semaphore>,
    learning_history: crate::learning_preview::HistoryCache,
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
        let selections: SelectionMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let owned_agent_handles: OwnedAgentHandles =
            Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
        let handles_for_resume = owned_agent_handles.clone();
        let retain_handle: Arc<dyn Fn(dsh_agent::AgentHandle) -> Arc<dyn Agent> + Send + Sync> =
            Arc::new(move |handle| {
                let agent = handle.agent.clone();
                handles_for_resume.lock().insert(
                    agent.id().clone(),
                    OwnedAgentHandle {
                        agent: agent.clone(),
                        dispose: Some(handle.dispose),
                    },
                );
                agent
            });
        let selection_setup = model_selection_setup(defaults.clone(), selections.clone());
        let setup_for_resume = selection_setup.clone();
        let ctx_for_resume = ctx.clone();
        let resolver = crate::agent_lookup::AgentResolver::new(
            ctx,
            crate::agent_lookup::ApiRemoteAgentOptions {
                agent_options,
                retain_handle,
                setup: Some(Arc::new(move |header, events| {
                    let selection = setup_for_resume.clone();
                    let presets = ctx_for_resume
                        .get_typed::<Arc<dsh_agent_presets::AgentPresets>>("agentPresets", false)
                        .map(|slot| slot.as_ref().clone());
                    let preset_id = dsh_agent_presets::resolve_session_preset(&header, &events);
                    Box::pin(async move {
                        Ok(Some(composed_agent_setup(selection, presets, preset_id)))
                    })
                })),
            },
        );
        let interactions = crate::interactions::InteractionState::new();
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            defaults,
            resolver,
            model_selection_setup: selection_setup,
            selections,
            owned_agent_handles,
            preset_switches: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            preset_switch_counter: std::sync::atomic::AtomicU64::new(0),
            interactions: interactions.clone(),
            history_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            learning_history: crate::learning_preview::HistoryCache::default(),
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

    fn retain_owned_handle(&self, handle: dsh_agent::AgentHandle) -> Arc<dyn Agent> {
        let agent = handle.agent.clone();
        self.owned_agent_handles.lock().insert(
            agent.id().clone(),
            OwnedAgentHandle {
                agent: agent.clone(),
                dispose: Some(handle.dispose),
            },
        );
        agent
    }

    fn spawn_idle_retirement(&self, agent: Arc<dyn Agent>) {
        let session_id = agent.id().clone();
        let admission = self.resolver.admission(&session_id);
        let resolver = Arc::clone(&self.resolver);
        let handles = Arc::clone(&self.owned_agent_handles);
        let sessions = self.sessions();
        let agents = self.agents();
        tokio::spawn(async move {
            agent.when_idle().await;
            let _admission = admission.lock().await;
            retire_idle_agent(&resolver, &handles, sessions, agents, agent).await;
        });
    }

    #[cfg(test)]
    async fn retire_idle_agent_for_test(&self, agent: Arc<dyn Agent>) {
        retire_idle_agent(
            &self.resolver,
            &self.owned_agent_handles,
            self.sessions(),
            self.agents(),
            agent,
        )
        .await;
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
        if let Some(rest) = error.strip_prefix("agent-presets: preset \"")
            && let Some((id, tail)) = rest.split_once('"')
        {
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
                home: self.defaults.dsh_home.clone(),
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
    async fn plugin_inventory_list(
        &self,
        request: RpcRequest<serde_json::Value>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(inventory) = self
            .ctx
            .get_typed::<Arc<dsh_host_plugin_inventory::PluginInventoryGateway>>(
                "pluginInventory",
                false,
            )
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "pluginInventory service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        ok(request.rpc_id, inventory.list().await)
    }

    async fn plugin_inventory_set_enabled(
        &self,
        request: RpcRequest<dsh_host_plugin_inventory::PluginSetEnabledRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(loader) = self
            .ctx
            .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "loader service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let _mutation = self.defaults.plugin_mutation_lock.lock().await;
        let Ok(entry) = loader.tree.resolve(&request.payload.entry_id) else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("unknown plugin entry {:?}", request.payload.entry_id),
                    details: EmptyDetails {},
                }),
            );
        };
        let previous = entry.options.lock().clone();
        let mut patch = indexmap::IndexMap::new();
        patch.insert(
            "disabled".to_string(),
            serde_json::Value::Bool(!request.payload.enabled),
        );
        if let Err(error) = entry.update(patch, false).await {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("plugin enablement failed: {error}"),
                    details: EmptyDetails {},
                }),
            );
        }
        if let Some(path) = &self.defaults.plugins_document {
            let entry_id = request.payload.entry_id.clone();
            let enabled = request.payload.enabled;
            let write = dsh_atomic_write::with_file_lock(path, async {
                let raw = tokio::fs::read(path).await?;
                let mut entries: Vec<serde_json::Value> = serde_json::from_slice(&raw)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                if !set_plugin_document_enabled(&mut entries, &entry_id, enabled) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("plugin entry {entry_id:?} is absent from the latest config"),
                    ));
                }
                let bytes = serde_json::to_vec_pretty(&entries)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                dsh_atomic_write::write_file_atomic(
                    path,
                    &bytes,
                    dsh_atomic_write::WriteFileAtomicOptions {
                        mode: 0o600,
                        dir_mode: Some(0o700),
                    },
                )
                .await
            })
            .await
            .and_then(|result| result);
            if let Err(error) = write {
                let rollback = entry.replace_options(previous).await;
                let message = match rollback {
                    Ok(()) => format!("plugin config persist failed: {error}"),
                    Err(rollback_error) => format!(
                        "plugin config persist failed: {error}; runtime rollback failed: {rollback_error}"
                    ),
                };
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message,
                        details: EmptyDetails {},
                    }),
                );
            }
        }
        let inventory = self
            .ctx
            .get_typed::<Arc<dsh_host_plugin_inventory::PluginInventoryGateway>>(
                "pluginInventory",
                false,
            )
            .map(|slot| slot.as_ref().clone())
            .expect("plugin inventory service");
        let snapshot = inventory.list().await;
        let selected = snapshot
            .entries
            .into_iter()
            .find(|entry| entry.entry_id.as_str() == request.payload.entry_id)
            .expect("updated entry remains in inventory");
        ok(
            request.rpc_id,
            dsh_host_plugin_inventory::PluginSetEnabledResult { entry: selected },
        )
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
        if request
            .payload
            .reference
            .to_ascii_uppercase()
            .starts_with("DSH_OAUTH_")
        {
            return err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message: "账号凭据请通过账号管理修改".into(),
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            );
        }

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
        if request
            .payload
            .reference
            .to_ascii_uppercase()
            .starts_with("DSH_OAUTH_")
        {
            return err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message: "账号凭据请通过账号管理修改".into(),
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            );
        }

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
        mutation: GoalMutation,
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
                    GoalVerb::Pause => {
                        let view = goals.pause(&agent, &goal_ref)?;
                        agent.cancel(
                            dsh_session::AgentCancelCause::User,
                            Some(&dsh_agent::CancelOptions { keep_inbox: true }),
                        );
                        Ok(view)
                    }
                    GoalVerb::Resume => goals.resume(&agent, &goal_ref),
                    GoalVerb::Complete => goals.complete(&agent, &goal_ref),
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
                        description: model.description,
                        api: model.api,
                        reasoning_default: model.reasoning_default,
                        effort_descriptions: model.effort_descriptions,
                        supports_reasoning_summaries: model.supports_reasoning_summaries,
                        supported_parameters: model.supported_parameters,
                        available: model.available,
                        reasoning_efforts: model.reasoning_efforts,
                        input: model.input,
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
    async fn learning_rpc(
        &self,
        rpc_id: RpcId,
        method: &str,
        payload: serde_json::Value,
    ) -> RpcResponse<serde_json::Value> {
        fn invalid(message: impl Into<String>) -> RpcError {
            RpcError::BadRequest(RpcErrorBody {
                message: message.into(),
                details: crate::api::rpc::BadRequestDetails { issues: vec![] },
            })
        }
        let Some(store) = self
            .ctx
            .get_typed::<Arc<dsh_tool_memory_local::learning::LearningStore>>(
                "learningStore",
                false,
            )
            .map(|slot| slot.as_ref().clone())
        else {
            return err(rpc_id, invalid("经验服务未就绪"));
        };
        if method == "memory.learningPreview" {
            if let Err(error) = store.flush_pending().await {
                return err(rpc_id, invalid(error));
            }
            let Some(id) = payload.get("sessionId").and_then(serde_json::Value::as_str) else {
                return err(rpc_id, invalid("缺少 sessionId"));
            };
            let id = dsh_session::session_id(id);
            let session = self.sessions().and_then(|sessions| sessions.get(&id));
            let agent = self.agents().and_then(|agents| agents.get(&id));
            let (basis, session_source) = if let Some(agent) = &agent {
                let Some(cwd) = agent
                    .session()
                    .header()
                    .cwd
                    .as_deref()
                    .filter(|cwd| !cwd.trim().is_empty())
                else {
                    return err(rpc_id, invalid("会话没有工作区"));
                };
                let selection = self.selection_for(agent).await.ok();
                let tools = self
                    .ctx
                    .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
                    .map(|slot| slot.as_ref().clone());
                (
                    crate::learning_preview::HistoryBasis {
                        cwd: cwd.into(),
                        provider: selection.as_ref().map(|value| value.provider.clone()),
                        model: selection.map(|value| value.model),
                        tools: tools
                            .as_ref()
                            .map(|tools| {
                                tools
                                    .schemas(Some(agent.scope_key()))
                                    .into_iter()
                                    .map(|tool| tool.name)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        tool_source: if tools.is_some() {
                            "current"
                        } else {
                            "unavailable"
                        },
                        model_source: "current-selection",
                        limited: false,
                    },
                    "live",
                )
            } else if let Some(session) = session {
                let Some(cwd) = session
                    .header()
                    .cwd
                    .as_deref()
                    .filter(|cwd| !cwd.trim().is_empty())
                else {
                    return err(rpc_id, invalid("会话没有工作区"));
                };
                let events = session.events();
                let start = events.len().saturating_sub(4096);
                (
                    crate::learning_preview::from_events(cwd.into(), &events[start..], start != 0),
                    "live",
                )
            } else {
                let Some(persistence) = self
                    .ctx
                    .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                        "sessionPersistence",
                        false,
                    )
                    .map(|slot| slot.as_ref().clone())
                else {
                    return err(rpc_id, invalid("会话持久化服务未就绪"));
                };
                match self.learning_history.read(persistence, &id).await {
                    Ok(basis) => (basis, "persisted"),
                    Err(error) => return err(rpc_id, invalid(error)),
                }
            };
            let notice = match basis.tool_source {
                "last-request" => {
                    "此预览基于上次请求的工具目录；下次执行会按实时目录重新计算，历史记录不表示工具当前已可用。"
                }
                "unavailable" => {
                    "有界历史片段中没有可核实的工具目录；未知工具不计为可用，下次请求将按实时目录重新计算。"
                }
                _ => "预览按当前模型选择与工具目录计算；实际请求会再次核对。",
            };
            let context = dsh_tool_memory_local::experience_reuse::ReuseContext {
                workspace: basis.cwd,
                provider: basis.provider,
                model: basis.model,
                session_id: Some(id.to_string()),
                tool_names: basis.tools,
            };
            let mut preview = dsh_tool_memory_local::experience_reuse::preview(
                &store,
                &context,
                dsh_tool_memory_local::experience_reuse::CONTEXT_BUDGET,
            );
            preview["toolSource"] = serde_json::json!(basis.tool_source);
            preview["modelSource"] = serde_json::json!(basis.model_source);
            preview["sessionSource"] = serde_json::json!(session_source);
            preview["historyLimited"] = serde_json::json!(basis.limited);
            preview["notice"] = serde_json::json!(notice);
            if basis.tool_source != "current" {
                preview["mode"] = serde_json::json!("historical-context-preview");
                preview["selectionRules"] = serde_json::json!([
                    "verified-and-enabled",
                    "same-workspace",
                    "historical-tool-snapshot-or-provider-model",
                    "fixed-template-or-user-confirmation",
                    "bounded-context"
                ]);
            }
            return ok(rpc_id, preview);
        }
        match store.invoke(method, payload).await {
            Ok(mut value) => {
                if let Some(items) = value
                    .get_mut("items")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    let labels: std::collections::HashMap<_, _> = self
                        .workspace_registry()
                        .and_then(|registry| registry.list().ok())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|workspace| {
                            let path = workspace.path();
                            let title = workspace.title();
                            (
                                dsh_tool_memory_local::learning::workspace_key(&path),
                                if title.trim().is_empty() { path } else { title },
                            )
                        })
                        .collect();
                    for item in items {
                        if let Some(label) = item
                            .get("workspaceKey")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|key| labels.get(key))
                        {
                            item["workspaceLabel"] = serde_json::Value::String(label.clone());
                        }
                    }
                }
                ok(rpc_id, value)
            }
            Err(error) => err(rpc_id, invalid(error)),
        }
    }

    fn memory_store(&self) -> Option<Arc<dsh_tool_memory_local::MemoryStore>> {
        self.ctx
            .get_typed::<Arc<dsh_tool_memory_local::MemoryStore>>("memoryStore", false)
            .map(|slot| slot.as_ref().clone())
    }

    async fn memory_rpc(
        &self,
        rpc_id: RpcId,
        method: &str,
        payload: serde_json::Value,
    ) -> RpcResponse<serde_json::Value> {
        let Some(store) = self.memory_store() else {
            return err(
                rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "memory service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        match method {
            "memory.categories" => ok(
                rpc_id,
                serde_json::json!({
                    "categories": dsh_tool_memory_local::BUILTIN_CATEGORIES.iter().map(|(id, label)| serde_json::json!({ "id": id, "label": label })).collect::<Vec<_>>()
                }),
            ),
            "memory.list" => ok(
                rpc_id,
                serde_json::json!({
                    "entries": store.list(payload.get("scope").and_then(|v| v.as_str()), payload.get("category").and_then(|v| v.as_str())).await
                }),
            ),
            "memory.upsert" => {
                let entry = match serde_json::from_value::<dsh_tool_memory_local::MemoryEntry>(
                    payload
                        .get("entry")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(entry) => entry,
                    Err(error) => return err(rpc_id, bad_request("memory.upsert", error)),
                };
                match store
                    .upsert(
                        entry,
                        payload.get("expectedRevision").and_then(|v| v.as_u64()),
                    )
                    .await
                {
                    Ok(entry) => ok(rpc_id, serde_json::json!({ "entry": entry })),
                    Err(message) => err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message,
                            details: EmptyDetails {},
                        }),
                    ),
                }
            }
            "memory.remove" => {
                let Some(id) = payload.get("id").and_then(|v| v.as_str()) else {
                    return err(
                        rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: "memory.remove: missing id".to_string(),
                            details: crate::api::rpc::BadRequestDetails { issues: Vec::new() },
                        }),
                    );
                };
                match store
                    .remove(id, payload.get("expectedRevision").and_then(|v| v.as_u64()))
                    .await
                {
                    Ok(removed) => ok(rpc_id, serde_json::json!({ "removed": removed })),
                    Err(message) => err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message,
                            details: EmptyDetails {},
                        }),
                    ),
                }
            }
            _ => unreachable!(),
        }
    }

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
        let opened = match &self.defaults.open_path {
            Some(open_path) => open_path(path.clone(), signal.clone()).await,
            None => {
                let abort = signal.clone();
                crate::native_path_opener::open_native_text_file(
                    &path,
                    Some(Arc::new(move || abort.aborted())),
                    &crate::native_path_opener::PathOpenerInternals::default(),
                )
                .await
                .map_err(|error| error.to_string())
            }
        };
        match opened {
            Ok(()) => ok(
                request.rpc_id,
                crate::api::settings::SettingsOpenDocumentResult { opened: true },
            ),
            Err(_error) if signal.aborted() => err(
                request.rpc_id,
                RpcError::Cancelled(RpcErrorBody {
                    message: "settings document open was aborted".to_string(),
                    details: EmptyDetails {},
                }),
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

    async fn workspace_delete_archived_session(
        &self,
        request: RpcRequest<crate::api::workspace::WorkspaceArchiveSessionRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let Some(registry) = self.workspace_registry() else {
            return err(request.rpc_id, Self::workspace_absent());
        };
        let session_id = dsh_session::session_id(request.payload.session_id.clone());
        let live = self.agents().and_then(|agents| agents.get(&session_id));
        let owned_dispose = {
            let mut handles = self.owned_agent_handles.lock();
            match handles.get_mut(&session_id) {
                Some(handle)
                    if live
                        .as_ref()
                        .is_some_and(|live| !Arc::ptr_eq(&handle.agent, live)) =>
                {
                    None
                }
                Some(handle) => handle
                    .dispose
                    .take()
                    .map(|dispose| (handle.agent.clone(), dispose)),
                None => None,
            }
        };
        if let Some(live_agent) = live.as_ref()
            && let Some(subagents) = self.subagents()
            && let Err(error) = subagents
                .drain_continuable_descendants(std::slice::from_ref(live_agent))
                .await
        {
            return err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: format!(
                        "session \"{session_id}\" continuable descendants could not be drained: {error}"
                    ),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "continuable descendant drain failed".to_string(),
                    },
                }),
            );
        }
        if let Some(live_agent) = live.as_ref()
            && owned_dispose.is_none()
        {
            let Some(agents) = self.agents() else {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: "agent registry disappeared while retiring a live session"
                            .to_string(),
                        details: EmptyDetails {},
                    }),
                );
            };
            if !agents.can_retire(live_agent) {
                return err(
                    request.rpc_id,
                    RpcError::AgentBusy(RpcErrorBody {
                        message: format!(
                            "session \"{session_id}\" is owned by another live subsystem"
                        ),
                        details: crate::api::rpc::ReasonDetails {
                            reason:
                                "the structural Agent factory does not own this exact lifecycle"
                                    .to_string(),
                        },
                    }),
                );
            }
            live_agent.cancel(dsh_agent::AgentCancelCause::User, None);
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                agents.retire(live_agent.clone()),
            )
            .await
            {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) | Err(_) => {
                    return err(
                        request.rpc_id,
                        RpcError::AgentBusy(RpcErrorBody {
                            message: format!(
                                "session \"{session_id}\" did not stop within 5 seconds; permanent deletion was not started"
                            ),
                            details: crate::api::rpc::ReasonDetails {
                                reason: "structural agent retirement did not settle".to_string(),
                            },
                        }),
                    );
                }
                Ok(Err(error)) => {
                    return err(
                        request.rpc_id,
                        RpcError::AgentBusy(RpcErrorBody {
                            message: format!(
                                "session \"{session_id}\" could not be retired: {error}"
                            ),
                            details: crate::api::rpc::ReasonDetails {
                                reason: "structural agent retirement failed".to_string(),
                            },
                        }),
                    );
                }
            }
        }
        if let Some((disposed_agent, mut dispose)) = owned_dispose {
            if tokio::time::timeout(std::time::Duration::from_secs(5), &mut dispose)
                .await
                .is_err()
            {
                let mut handles = self.owned_agent_handles.lock();
                if let Some(current) = handles.get_mut(&session_id)
                    && current.dispose.is_none()
                    && Arc::ptr_eq(&current.agent, &disposed_agent)
                {
                    current.dispose = Some(dispose);
                }
                return err(
                    request.rpc_id,
                    RpcError::AgentBusy(RpcErrorBody {
                        message: format!(
                            "session \"{session_id}\" did not stop within 5 seconds; permanent deletion was not started"
                        ),
                        details: crate::api::rpc::ReasonDetails {
                            reason: "agent disposal timed out".to_string(),
                        },
                    }),
                );
            }
            let mut handles = self.owned_agent_handles.lock();
            if handles.get(&session_id).is_some_and(|current| {
                current.dispose.is_none() && Arc::ptr_eq(&current.agent, &disposed_agent)
            }) {
                handles.remove(&session_id);
            }
        }
        match registry.delete_archived_session(&session_id, None).await {
            Ok(_artifact_existed) => ok(
                request.rpc_id,
                crate::api::workspace::WorkspaceArchiveSessionResult {
                    // Success means the session is now durably absent. An
                    // already-unmaterialized artifact is still a successful
                    // permanent deletion.
                    deleted: true,
                    archived_session_ids: registry
                        .archived_session_ids()
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect(),
                },
            ),
            Err(error) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("workspace.deleteArchivedSession: {error}"),
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
                    deleted: false,
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
        projections: Option<&dsh_session_projection::SessionProjectionRegistry>,
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
            projections: projections.map(|registry| {
                let snapshot = registry.snapshot(session);
                crate::api::sessions::SessionProjectionsBlock {
                    as_of_seq: snapshot.as_of_seq,
                    values: serde_json::Value::Object(snapshot.values),
                }
            }),
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
        let projection_registry = self
            .ctx
            .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                "sessionProjections",
                false,
            )
            .map(|slot| slot.as_ref().clone());
        let mut items: Vec<SessionSummary> = sessions
            .list()
            .iter()
            .map(|session| self.summarize_attached(session, projection_registry.as_deref()))
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
            let cache = self
                .ctx
                .get_typed::<Arc<dsh_session_projection_cache::SessionProjectionCache>>(
                    "sessionProjectionCache",
                    false,
                )
                .map(|slot| slot.as_ref().clone());
            let cold = match persistence.list().await {
                Ok(cold) => cold,
                Err(error) => {
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!("session.list: {error}"),
                            details: EmptyDetails {},
                        }),
                    );
                }
            };
            for meta in cold {
                if attached.contains(meta.id.as_str())
                    || meta.cwd.is_none()
                    || meta.version != dsh_session::SESSION_FORMAT_VERSION
                {
                    continue;
                }
                let snapshot = match cache.as_ref() {
                    Some(cache) => {
                        let cached = cache.cached_snapshot(&meta).filter(|snapshot| {
                            snapshot
                                .values
                                .contains_key(dsh_session_title::SESSION_LIST_METADATA_KEY)
                        });
                        match cached {
                            Some(snapshot) => Some(snapshot),
                            None => match cache.cold_snapshot(&meta.id).await {
                                Ok(snapshot) => Some(snapshot),
                                Err(error) => {
                                    return err(
                                        request.rpc_id,
                                        RpcError::Internal(RpcErrorBody {
                                            message: format!(
                                                "session.list: cannot read persisted projection {}: {error}",
                                                meta.id.as_str()
                                            ),
                                            details: EmptyDetails {},
                                        }),
                                    );
                                }
                            },
                        }
                    }
                    None => None,
                };
                let metadata = snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .values
                        .get(dsh_session_title::SESSION_LIST_METADATA_KEY)
                });
                let blank = metadata
                    .and_then(|value| value.get("blank"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                let updated_at = metadata
                    .and_then(|value| value.get("updatedAt"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(meta.created_at as i64);
                items.push(SessionSummary {
                    session_id: meta.id.clone(),
                    updated_at,
                    running: false,
                    blank,
                    parent_session_id: meta.parent_session.clone(),
                    origin: meta.origin.as_deref().and_then(|origin| match origin {
                        "subagent" => Some(crate::api::sessions::SessionOrigin::Subagent),
                        _ => None,
                    }),
                    cwd: meta.cwd.clone(),
                    agent_preset: meta.agent_preset.clone(),
                    projections: snapshot.map(|snapshot| {
                        crate::api::sessions::SessionProjectionsBlock {
                            as_of_seq: snapshot.as_of_seq,
                            values: serde_json::Value::Object(snapshot.values),
                        }
                    }),
                });
            }
        }
        // updatedAt descending (the TS sort).
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
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

        let Some(_sessions) = self.sessions() else {
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
        let workspace = match request.payload.workspace_id.as_ref() {
            Some(workspace_id) => {
                let Some(registry) = self.workspace_registry() else {
                    return err(request.rpc_id, Self::workspace_absent());
                };
                let workspace_id = dsh_workspace::workspace_id(workspace_id.to_string());
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
                Some(workspace)
            }
            None => None,
        };
        let cwd = request
            .payload
            .cwd
            .clone()
            .or_else(|| workspace.as_ref().map(|workspace| workspace.path()))
            .unwrap_or_else(|| self.defaults.cwd.clone());
        let session_id = request.payload.session_id.clone();
        let presets = self.agent_presets();
        let resolved_preset = if let Some(presets) = presets.as_ref() {
            match presets
                .resolve(request.payload.agent_preset.as_deref())
                .await
            {
                Ok(preset) => Some(preset.id),
                Err(error) => return self.preset_failure_unknown(request.rpc_id, error),
            }
        } else {
            None
        };
        if let Some(existing_id) = session_id.as_ref() {
            match self.resolver.resolve(existing_id).await {
                crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => {
                    let existing_cwd = agent.session().header().cwd.clone();
                    if existing_cwd.as_deref() != Some(cwd.as_str()) {
                        return err(
                            request.rpc_id,
                            RpcError::SessionConflict(RpcErrorBody {
                                message: format!(
                                    "session \"{existing_id}\" already exists with a different cwd"
                                ),
                                details: crate::api::rpc::SessionConflictDetails {
                                    session_id: existing_id.to_string(),
                                    requested_cwd: cwd,
                                    existing_cwd,
                                },
                            }),
                        );
                    }
                    let existing_preset = dsh_agent_presets::resolve_session_preset(
                        agent.session().header(),
                        &agent.session().events(),
                    );
                    if request.payload.agent_preset.is_some() && existing_preset != resolved_preset
                    {
                        return err(
                            request.rpc_id,
                            RpcError::AgentPresetConflict(RpcErrorBody {
                                message: format!(
                                    "session \"{existing_id}\" already uses a different agent preset"
                                ),
                                details: crate::api::rpc::AgentPresetConflictDetails {
                                    session_id: existing_id.to_string(),
                                    requested_preset: resolved_preset.clone().unwrap_or_default(),
                                    existing_preset,
                                },
                            }),
                        );
                    }
                    if let Some(workspace) = workspace
                        && let Err(error) = workspace.attach_session(existing_id).await
                    {
                        return err(
                            request.rpc_id,
                            RpcError::Internal(RpcErrorBody {
                                message: format!(
                                    "session.create: workspace attach failed: {error}"
                                ),
                                details: EmptyDetails {},
                            }),
                        );
                    }
                    return ok(
                        request.rpc_id,
                        crate::api::sessions::SessionCreateResult {
                            session_id: existing_id.clone(),
                            agent_preset: dsh_agent_presets::resolve_session_preset(
                                agent.session().header(),
                                &agent.session().events(),
                            ),
                        },
                    );
                }
                crate::agent_lookup::ApiRemoteAgentResult::Error(error)
                    if error.code() != crate::api::rpc::RpcErrorCode::SessionNotFound =>
                {
                    return err(request.rpc_id, error);
                }
                crate::agent_lookup::ApiRemoteAgentResult::Error(_) => {}
            }
        }
        let meta = CreateSessionMeta {
            cwd: Some(cwd),
            agent_preset: resolved_preset.clone(),
            ..Default::default()
        };
        // The agent factory owns the one session creation transaction. Creating
        // through SessionStore first would make the factory create the same id
        // twice and leave a half-published session after the error.
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
                session_id: session_id.clone(),
                meta: Some(meta),
                agent_options: Some(agent_options),
                setup: Some(composed_agent_setup(
                    self.model_selection_setup.clone(),
                    presets,
                    resolved_preset,
                )),
                ..Default::default()
            })
            .await
        {
            Ok(handle) => {
                let session = handle.agent.session().clone();
                if let Some(workspace) = workspace
                    && let Err(error) = workspace.attach_session(session.id()).await
                {
                    handle.dispose.await;
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!("session.create: workspace attach failed: {error}"),
                            details: EmptyDetails {},
                        }),
                    );
                }
                let _agent = self.retain_owned_handle(handle);
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
        if tokio::time::timeout(std::time::Duration::from_secs(10), agent.when_idle())
            .await
            .is_err()
        {
            return err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: format!(
                        "session \"{}\" did not become idle after cancellation",
                        request.payload.session_id
                    ),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "cancel-timeout".to_string(),
                    },
                }),
            );
        }
        ok(
            request.rpc_id,
            crate::api::sessions::AcceptedResult { accepted: true },
        )
    }
    /// Select a bounded, message-aligned history page without first cloning
    /// the whole log. If the requested message count spans too many raw
    /// events, reduce the count until a safe contiguous page fits.
    fn paginate(
        events: &[dsh_session::SessionEvent],
        before_seq: Option<i64>,
        max_messages: u64,
    ) -> Result<(Vec<dsh_session::SessionEvent>, bool), usize> {
        const MAX_HISTORY_EVENTS: usize = 4_096;
        let before_seq = before_seq.and_then(|value| u64::try_from(value).ok());
        let mut messages = max_messages.max(1);
        loop {
            match dsh_session_persistence::select_history_window(
                events,
                before_seq,
                messages,
                HISTORY_SCAN_EVENT_LIMIT,
            ) {
                Ok(selection) => {
                    let source = &events[selection.start..selection.end];
                    if Self::compact_history_bytes(source) <= HISTORY_SOURCE_BYTE_LIMIT {
                        let compact =
                            crate::api::sessions::coalesce_history_transport_slice(source);
                        if compact.len() <= MAX_HISTORY_EVENTS
                            && Self::compact_history_bytes(&compact) <= HISTORY_TRANSPORT_BYTE_LIMIT
                        {
                            return Ok((compact, selection.has_more));
                        }
                    }
                    if messages > 1 {
                        messages = (messages / 2).max(1);
                        continue;
                    }
                    return Err(selection.event_count());
                }
                Err(_) if messages > 1 => messages = (messages / 2).max(1),
                Err(error) => return Err(error.selection.event_count()),
            }
        }
    }

    /// Select a bounded forward page starting at an indexed event. The target
    /// remains at the head so a jump reveals subsequent conversation first.
    fn paginate_forward(
        events: &[dsh_session::SessionEvent],
        after_seq: i64,
        max_messages: u64,
    ) -> Result<(Vec<dsh_session::SessionEvent>, bool), usize> {
        const MAX_HISTORY_EVENTS: usize = HISTORY_SCAN_EVENT_LIMIT;
        let after_seq = u64::try_from(after_seq).unwrap_or(0);
        let start = events.partition_point(|event| event.seq.get() < after_seq);
        let mut messages = 0_u64;
        let mut end = events.len();
        for (offset, event) in events[start..].iter().enumerate() {
            if matches!(event.type_.as_str(), "user/message" | "assistant/message")
                && event.surface_op.as_ref().is_none_or(|op| op.is_append())
            {
                messages += 1;
                if messages >= max_messages.max(1) {
                    end = start + offset + 1;
                    break;
                }
            }
        }
        let required = end.saturating_sub(start);
        if required > MAX_HISTORY_EVENTS {
            end = start.saturating_add(MAX_HISTORY_EVENTS).min(events.len());
        }
        let source = &events[start..end];
        if Self::compact_history_bytes(source) > HISTORY_SOURCE_BYTE_LIMIT {
            return Err(source.len());
        }
        let compact = crate::api::sessions::coalesce_history_transport_slice(source);
        if compact.len() > HISTORY_TRANSPORT_EVENT_LIMIT
            || Self::compact_history_bytes(&compact) > HISTORY_TRANSPORT_BYTE_LIMIT
        {
            return Err(source.len());
        }
        Ok((compact, end < events.len()))
    }

    fn compact_history_bytes(events: &[dsh_session::SessionEvent]) -> usize {
        events
            .iter()
            .map(crate::api::sessions::serialized_event_len)
            .sum()
    }

    async fn read_cold_forward_compact(
        persistence: &Arc<dyn dsh_session_persistence::SessionPersistenceApi>,
        session_id: &dsh_session::SessionId,
        from_seq: u64,
        max_messages: u64,
    ) -> Result<
        (
            Vec<dsh_session::SessionEvent>,
            bool,
            dsh_session::SessionHeader,
        ),
        String,
    > {
        const MAX_COMPACT_BYTES: usize = 8 * 1024 * 1024;
        let window = persistence
            .read_forward_window(
                session_id,
                dsh_session_persistence::SessionReadForwardWindowRequest {
                    after_seq: from_seq,
                    max_messages,
                    max_events: HISTORY_SCAN_EVENT_LIMIT,
                },
            )
            .await?;
        if Self::compact_history_bytes(&window.events) > HISTORY_SOURCE_BYTE_LIMIT {
            return Err("history scan exceeds the 64 MiB source budget".into());
        }
        let has_more = window.has_more;
        let meta = window.meta;
        let mut compact = crate::api::sessions::coalesce_history_transport_events(window.events);
        if compact.len() > HISTORY_TRANSPORT_EVENT_LIMIT
            || Self::compact_history_bytes(&compact) > MAX_COMPACT_BYTES
        {
            return Err(format!(
                "session.history: targeted compact window exceeds the {MAX_COMPACT_BYTES} byte budget"
            ));
        }
        if let Some(first) = compact.first_mut() {
            first.data["__historyStartSeq"] = serde_json::Value::from(from_seq);
        }
        Ok((compact, has_more, meta))
    }

    async fn read_cold_tail_compact(
        persistence: &Arc<dyn dsh_session_persistence::SessionPersistenceApi>,
        session_id: &dsh_session::SessionId,
        max_messages: u64,
    ) -> Result<
        (
            Vec<dsh_session::SessionEvent>,
            bool,
            dsh_session::SessionHeader,
        ),
        String,
    > {
        const MAX_COMPACT_EVENTS: usize = 4_096;
        const MAX_COMPACT_BYTES: usize = 8 * 1024 * 1024;
        let window = persistence
            .read_window(
                session_id,
                dsh_session_persistence::SessionReadWindowRequest {
                    before_seq: None,
                    max_messages,
                    max_events: HISTORY_SCAN_EVENT_LIMIT,
                },
            )
            .await?;
        if let Some(required) = window.oversized_event_count {
            return Err(format!(
                "session.history: one safe tail group requires {required} events, above the {MAX_COMPACT_EVENTS} event budget"
            ));
        }
        if Self::compact_history_bytes(&window.events) > HISTORY_SOURCE_BYTE_LIMIT {
            return Err("history scan exceeds the 64 MiB source budget".into());
        }
        let compact = crate::api::sessions::coalesce_history_transport_events(window.events);
        if compact.len() > HISTORY_TRANSPORT_EVENT_LIMIT
            || Self::compact_history_bytes(&compact) > MAX_COMPACT_BYTES
        {
            return Err(format!(
                "session.history: compact tail exceeds the {MAX_COMPACT_BYTES} byte budget"
            ));
        }
        Ok((compact, window.has_more, window.meta))
    }

    async fn session_history(
        &self,
        request: RpcRequest<crate::api::sessions::SessionHistoryRequest>,
    ) -> RpcResponse<serde_json::Value> {
        use crate::api::sessions::HistoryEntry;

        let _history_permit = self
            .history_gate
            .acquire()
            .await
            .expect("history semaphore remains open for the service lifetime");
        let session_id = request.payload.session_id.clone();
        const DEFAULT_MAX_MESSAGES: u64 = 100;
        const MAX_HISTORY_EVENTS: usize = 4_096;
        let requested_messages = request
            .payload
            .max_messages
            .unwrap_or(DEFAULT_MAX_MESSAGES)
            .max(1);
        if request.payload.before_seq.is_some() && request.payload.after_seq.is_some() {
            return err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message: "session.history beforeSeq and afterSeq are mutually exclusive"
                        .to_string(),
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            );
        }
        // Attached sessions already own their log; select by index and clone
        // only the bounded page. Cold sessions delegate the same contract to
        // persistence so the full log is never materialized in ApiProxy.
        let mut live_session: Option<dsh_session::Session> = None;
        let mut cold_header: Option<dsh_session::SessionHeader> = None;
        let (page_events, has_more) = match self.sessions().and_then(|store| store.get(&session_id))
        {
            Some(session) => {
                live_session = Some(session.clone());
                let selected = if let Some(after_seq) = request.payload.after_seq {
                    Self::paginate_forward(
                        session.events().as_slice(),
                        after_seq,
                        requested_messages,
                    )
                } else {
                    Self::paginate(
                        session.events().as_slice(),
                        request.payload.before_seq,
                        requested_messages,
                    )
                };
                match selected {
                    Ok(page) => page,
                    Err(required) => {
                        return err(
                            request.rpc_id,
                            RpcError::Internal(RpcErrorBody {
                                message: format!(
                                    "session.history: one safe history group requires {required} events, above the {MAX_HISTORY_EVENTS} event budget"
                                ),
                                details: EmptyDetails {},
                            }),
                        );
                    }
                }
            }
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
                if let Some(after_seq) = request.payload.after_seq {
                    let from_seq = u64::try_from(after_seq).unwrap_or(0);
                    let (events, has_more, meta) = match Self::read_cold_forward_compact(
                        &persistence,
                        &session_id,
                        from_seq,
                        requested_messages,
                    )
                    .await
                    {
                        Ok(compact) => compact,
                        Err(error) => {
                            return err(
                                request.rpc_id,
                                history_read_error(session_id.as_str(), error),
                            );
                        }
                    };
                    cold_header = Some(meta);
                    (events, has_more)
                } else if request.payload.before_seq.is_none() {
                    let (events, has_more, meta) = match Self::read_cold_tail_compact(
                        &persistence,
                        &session_id,
                        requested_messages,
                    )
                    .await
                    {
                        Ok(compact) => compact,
                        Err(error) => {
                            return err(
                                request.rpc_id,
                                history_read_error(session_id.as_str(), error),
                            );
                        }
                    };
                    cold_header = Some(meta);
                    (events, has_more)
                } else {
                    let before_seq = request
                        .payload
                        .before_seq
                        .and_then(|value| u64::try_from(value).ok());
                    let mut messages = requested_messages;
                    loop {
                        let window = match persistence
                            .read_window(
                                &session_id,
                                dsh_session_persistence::SessionReadWindowRequest {
                                    before_seq,
                                    max_messages: messages,
                                    max_events: HISTORY_SCAN_EVENT_LIMIT,
                                },
                            )
                            .await
                        {
                            Ok(window) => window,
                            Err(error) => {
                                return err(
                                    request.rpc_id,
                                    history_read_error(session_id.as_str(), error),
                                );
                            }
                        };
                        if let Some(required) = window.oversized_event_count {
                            if messages > 1 {
                                let proportional = messages
                                    .saturating_mul(MAX_HISTORY_EVENTS as u64)
                                    .checked_div(required as u64)
                                    .unwrap_or(1)
                                    .max(1);
                                messages = proportional.min(messages - 1);
                                continue;
                            }
                            return err(
                                request.rpc_id,
                                RpcError::Internal(RpcErrorBody {
                                    message: format!(
                                        "session.history: one safe history group requires {required} events, above the {MAX_HISTORY_EVENTS} event budget"
                                    ),
                                    details: EmptyDetails {},
                                }),
                            );
                        }
                        cold_header = Some(window.meta);
                        break (window.events, window.has_more);
                    }
                }
            }
        };
        let presentation_scope = if let Some(agent) =
            self.agents().and_then(|agents| agents.get(&session_id))
        {
            Some(agent.scope_key().clone())
        } else if let (Some(header), Some(presets)) = (cold_header.as_ref(), self.agent_presets()) {
            let preset = dsh_agent_presets::resolve_session_preset(header, &page_events);
            presets.standing_key_for(preset.as_deref()).await.ok()
        } else {
            None
        };
        let tools = self
            .ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone());
        let first_seq = page_events.first().and_then(|event| {
            i64::try_from(
                event
                    .data
                    .get("__historyStartSeq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(event.seq.get()),
            )
            .ok()
        });
        let last_seq = page_events.last().and_then(|event| {
            i64::try_from(
                event
                    .data
                    .get("__historyEndSeq")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(event.seq.get()),
            )
            .ok()
        });
        let page_events = if live_session.is_some() || request.payload.before_seq.is_some() {
            crate::api::sessions::coalesce_history_transport_events(page_events)
        } else {
            page_events
        };
        if page_events.len() > HISTORY_TRANSPORT_EVENT_LIMIT
            || Self::compact_history_bytes(&page_events) > HISTORY_TRANSPORT_BYTE_LIMIT
        {
            return err(
                request.rpc_id,
                history_read_error(
                    session_id.as_str(),
                    "history transport window exceeds its event or byte budget".into(),
                ),
            );
        }
        let page: Vec<HistoryEntry> = page_events
            .into_iter()
            .map(|event| {
                let view = if event.type_ == "tool/call" {
                    let name = event.data.get("name").and_then(serde_json::Value::as_str);
                    let arguments = event
                        .data
                        .get("arguments")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
                    match (tools.as_ref(), name, arguments.as_ref()) {
                        (Some(tools), Some(name), Some(arguments)) => tools
                            .present_call_for_scope(presentation_scope.as_ref(), name, arguments)
                            .map(|view| crate::api::events::ToolEventView::Call { view }),
                        _ => None,
                    }
                } else {
                    None
                };
                HistoryEntry { event, view }
            })
            .collect();
        let has_more_before = if request.payload.after_seq.is_some() {
            first_seq.is_some_and(|seq| seq > 0)
        } else {
            has_more
        };
        let has_more_after = if request.payload.after_seq.is_some() {
            has_more
        } else {
            request.payload.before_seq.is_some()
        };
        let projections =
            if request.payload.before_seq.is_some() || request.payload.after_seq.is_some() {
                None
            } else if let Some(session) = live_session {
                self.ctx
                    .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                        "sessionProjections",
                        false,
                    )
                    .map(|registry| {
                        let snapshot = registry.snapshot(&session);
                        crate::api::sessions::SessionProjectionsBlock {
                            as_of_seq: snapshot.as_of_seq,
                            values: serde_json::Value::Object(snapshot.values),
                        }
                    })
            } else if let Some(cache) = self
                .ctx
                .get_typed::<Arc<dsh_session_projection_cache::SessionProjectionCache>>(
                    "sessionProjectionCache",
                    false,
                )
                .map(|slot| slot.as_ref().clone())
            {
                cache.cold_snapshot(&session_id).await.ok().map(|snapshot| {
                    crate::api::sessions::SessionProjectionsBlock {
                        as_of_seq: snapshot.as_of_seq,
                        values: serde_json::Value::Object(snapshot.values),
                    }
                })
            } else {
                None
            };
        ok(
            request.rpc_id,
            crate::api::sessions::SessionHistoryResult {
                has_more_before,
                has_more_after,
                first_seq,
                last_seq,
                events: page,
                has_more,
                projections,
            },
        )
    }
    /// Install (once) and return the selection state owned by this exact live
    /// Agent. Directly-registered test/deployment Agents are supported through
    /// the same lazy path; factory-created Agents install it before publication.
    async fn selection_state_for(&self, agent: &Arc<dyn Agent>) -> Result<SelectionState, String> {
        if let Some(commit) = (self.model_selection_setup)(agent.ctx(), agent.clone()).await? {
            commit.commit();
        }
        let entry = self
            .selections
            .lock()
            .get(agent.id())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "model selection was not installed for agent \"{}\"",
                    agent.id()
                )
            })?;
        let exact = entry
            .agent
            .upgrade()
            .is_some_and(|installed| Arc::ptr_eq(&installed, agent));
        if !exact {
            return Err(format!(
                "model selection belongs to a different agent for session \"{}\"",
                agent.id()
            ));
        }
        Ok(entry.state.clone())
    }

    async fn selection_for(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> Result<crate::api::sessions::ModelSelection, String> {
        let state = self.selection_state_for(agent).await?;
        state
            .lock()
            .resolved_current()
            .map(wire_selection)
            .ok_or_else(|| format!("agent \"{}\" has no model selection", agent.id()))
    }

    async fn session_models(
        &self,
        request: RpcRequest<crate::api::sessions::SessionRefRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let session_id = request.payload.session_id.clone();
        let Some(runtime) = self.llm_runtime() else {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: "session.models: the llm service is not composed".to_string(),
                    details: EmptyDetails {},
                }),
            );
        };
        let current = if let Some(agent) = self.agents().and_then(|agents| agents.get(&session_id))
        {
            match self.selection_for(&agent).await {
                Ok(current) => current,
                Err(error) => {
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!("session.models: {error}"),
                            details: EmptyDetails {},
                        }),
                    );
                }
            }
        } else {
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
                    RpcError::Internal(RpcErrorBody {
                        message: "session.models: session persistence is not composed".to_string(),
                        details: EmptyDetails {},
                    }),
                );
            };
            let state = match persistence.read_model_selection_state(&session_id).await {
                Ok(state) => state,
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
            };
            state
                .and_then(|state| state.get("selection").cloned())
                .filter(|selection| !selection.is_null())
                .and_then(|selection| {
                    serde_json::from_value::<crate::api::sessions::ModelSelection>(selection).ok()
                })
                .unwrap_or_else(|| (self.defaults.default_model_selection)())
        };
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
                        .map(dsh_llm::ReasoningEffortId::new),
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
        let pending_images = agent
            .inbox()
            .next_turn()
            .into_iter()
            .chain(agent.inbox().next_step())
            .any(|message| {
                message
                    .content
                    .iter()
                    .any(|block| matches!(block, dsh_llm::ContentBlock::Image { .. }))
            });
        if pending_images {
            let explicitly_rejects_images = runtime
                .resolve_model_info(&selected.provider, &selected.model, None)
                .await
                .is_ok_and(|model| {
                    model.input_modalities.as_ref().is_some_and(|modalities| {
                        !modalities.contains(&dsh_llm::ModelModality::Image)
                    })
                });
            if explicitly_rejects_images {
                return err(
                    request.rpc_id,
                    RpcError::AttachmentError(RpcErrorBody {
                        message: format!(
                            "model {}/{} does not declare image input support",
                            selected.provider, selected.model
                        ),
                        details: crate::api::rpc::ReasonDetails {
                            reason: "MODEL_DOES_NOT_SUPPORT_IMAGES".to_string(),
                        },
                    }),
                );
            }
        }
        let state = match self.selection_state_for(&agent).await {
            Ok(state) => state,
            Err(error) => {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!("session.selectModel: {error}"),
                        details: EmptyDetails {},
                    }),
                );
            }
        };
        if let Err(error) = agent.session().append(
            "model/selection",
            serde_json::to_value(&selected).expect("model selection serializes"),
            None,
        ) {
            return err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("session.selectModel: could not persist selection: {error}"),
                    details: EmptyDetails {},
                }),
            );
        }
        state.lock().current = Some(core_selection(selected.clone()));
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
        let last_seq = events
            .last()
            .map(|event| event.seq.get() as i64)
            .unwrap_or(-1);
        let at_seq = request.payload.at_seq;
        // An in-log anchor belongs to the turn containing it; omitted and
        // past-end anchors retain the last-completed-turn shortcut.
        let anchored_boundary = at_seq.and_then(|at| {
            events
                .iter()
                .find(|event| event.type_ == "turn/end" && (event.seq.get() as i64) >= at)
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
        let inherited_preset = dsh_agent_presets::resolve_session_preset(&header, &seed);
        let presets = self.agent_presets();
        let resolved_preset = if let Some(presets) = presets.as_ref() {
            match presets.resolve(inherited_preset.as_deref()).await {
                Ok(preset) => Some(preset.id),
                Err(error) => return self.preset_failure_unknown(request.rpc_id, error),
            }
        } else {
            None
        };
        let meta = dsh_session::CreateSessionMeta {
            cwd: header.cwd.clone(),
            parent_session: Some(session_id.clone()),
            is_seeded: Some(true),
            agent_preset: resolved_preset.clone(),
            ..Default::default()
        };
        let inherited_event_count = match dsh_session::SessionLogOffset::new(seed.len() as u64) {
            Ok(value) => value,
            Err(error) => {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!("forked session seed is too large: {error}"),
                        details: EmptyDetails {},
                    }),
                );
            }
        };
        match agents
            .create(dsh_agent::CreateAgentOptions {
                session_id: Some(child_id.clone()),
                seed: Some(seed),
                inherited_event_count: Some(inherited_event_count),
                meta: Some(meta),
                agent_options: Some(agent_options),
                setup: Some(composed_agent_setup(
                    self.model_selection_setup.clone(),
                    presets,
                    resolved_preset,
                )),
            })
            .await
        {
            Ok(handle) => {
                if let Some(registry) = self.workspace_registry() {
                    let source_workspace = registry.list().ok().and_then(|workspaces| {
                        workspaces
                            .into_iter()
                            .find(|workspace| workspace.session_ids().contains(&session_id))
                    });
                    if let Some(workspace) = source_workspace
                        && let Err(error) = workspace.attach_session(&child_id).await
                    {
                        handle.dispose.await;
                        return err(
                            request.rpc_id,
                            RpcError::Internal(RpcErrorBody {
                                message: format!(
                                    "forked session \"{child_id}\" could not inherit its workspace: {error}"
                                ),
                                details: EmptyDetails {},
                            }),
                        );
                    }
                }
                self.retain_owned_handle(handle);
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
    async fn session_update_todos(
        &self,
        request: RpcRequest<crate::api::sessions::SessionUpdateTodosRequest>,
    ) -> RpcResponse<serde_json::Value> {
        let session_id = request.payload.session_id.clone();
        let resolved = self.resolver.resolve(&session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        if crate::agent_lookup::has_api_remote_subagent_owner(
            &self.ctx,
            agent.session().header(),
            Some(&agent),
        ) {
            return err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: format!("session \"{session_id}\" is owned by subagent routing"),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "todo editing is unavailable for routed child sessions".to_string(),
                    },
                }),
            );
        }
        let mut replacement = request.payload.expected.clone();
        let (was_active, notice) = match &request.payload.action {
            crate::api::sessions::TodoAction::Edit { index, content } => {
                let Some(todo) = replacement.get_mut(*index) else {
                    return err(
                        request.rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: "task index is no longer valid".to_string(),
                            details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                        }),
                    );
                };
                let previous = todo.content.clone();
                todo.content = content.trim().to_string();
                (
                    todo.status == dsh_session::TodoStatus::InProgress,
                    format!(
                        "The user changed task {} from {} to {}. Follow the updated task list.",
                        index + 1,
                        serde_json::to_string(&previous).expect("task content"),
                        serde_json::to_string(&todo.content).expect("task content"),
                    ),
                )
            }
            crate::api::sessions::TodoAction::Remove { index } => {
                if *index >= replacement.len() {
                    return err(
                        request.rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: "task index is no longer valid".to_string(),
                            details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                        }),
                    );
                }
                let removed = replacement.remove(*index);
                if removed.status == dsh_session::TodoStatus::InProgress
                    && !replacement
                        .iter()
                        .any(|todo| todo.status == dsh_session::TodoStatus::InProgress)
                    && let Some(next) = replacement
                        .iter_mut()
                        .find(|todo| todo.status == dsh_session::TodoStatus::Pending)
                {
                    next.status = dsh_session::TodoStatus::InProgress;
                }
                (
                    removed.status == dsh_session::TodoStatus::InProgress,
                    format!(
                        "The user stopped and removed task {}: {}. Do not continue it; follow the updated task list.",
                        index + 1,
                        serde_json::to_string(&removed.content).expect("task content"),
                    ),
                )
            }
        };
        match dsh_tool_todo::replace_if_current(
            agent.session(),
            &request.payload.expected,
            &replacement,
            true,
        ) {
            Ok(_) => {
                let message = dsh_llm::create_user_message(
                    vec![dsh_llm::ContentBlock::Text { text: notice }],
                    dsh_llm::MessageSource::Plugin {
                        plugin: "@deepseek-ai/dsh-client-ui-conversation".to_string(),
                        form: Some(dsh_llm::ContextForm::Notice),
                        sections: None,
                        summary: Some("Task list changed by the user".to_string()),
                        compaction_id: None,
                        source_command_id: None,
                    },
                );
                if was_active && agent.status() == dsh_agent::AgentStatus::Running {
                    agent.steer(message);
                } else {
                    agent.inject(message);
                }
                ok(
                    request.rpc_id,
                    crate::api::sessions::AcceptedResult { accepted: true },
                )
            }
            Err(dsh_tool_todo::ReplaceTodosError::Conflict { .. }) => err(
                request.rpc_id,
                RpcError::AgentBusy(RpcErrorBody {
                    message: "the task list changed before this edit could be applied; reopen it and retry".to_string(),
                    details: crate::api::rpc::ReasonDetails {
                        reason: "todo-list-changed".to_string(),
                    },
                }),
            ),
            Err(dsh_tool_todo::ReplaceTodosError::Invalid(message)) => err(
                request.rpc_id,
                RpcError::BadRequest(RpcErrorBody {
                    message,
                    details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                }),
            ),
            Err(dsh_tool_todo::ReplaceTodosError::Append(message)) => err(
                request.rpc_id,
                RpcError::Internal(RpcErrorBody {
                    message: format!("session.updateTodos failed: {message}"),
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
        if let QueueAction::Edit { content } = &request.payload.action
            && content
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
            .any(|message| message.id == item_id);
        let in_step = inbox
            .next_step()
            .iter()
            .any(|message| message.id == item_id);
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
                .find(|message| message.id == item_id)
        } else {
            inbox
                .next_step()
                .into_iter()
                .find(|message| message.id == item_id)
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
        let admission = self
            .resolver
            .admission(&request.payload.session_id)
            .lock_owned()
            .await;
        let resolved = self.resolver.resolve(&request.payload.session_id).await;
        let agent = match resolved {
            crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
            crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                return err(request.rpc_id, error);
            }
        };
        if request
            .payload
            .content
            .iter()
            .any(|part| matches!(part, PromptContentPart::Image { .. }))
        {
            let selection = match self.selection_for(&agent).await {
                Ok(selection) => selection,
                Err(error) => {
                    return err(
                        request.rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: format!("session.prompt: {error}"),
                            details: EmptyDetails {},
                        }),
                    );
                }
            };
            let explicitly_rejects_images =
                if selection.provider.is_empty() || selection.model.is_empty() {
                    false
                } else {
                    match self.llm_runtime() {
                        Some(runtime) => runtime
                            .resolve_model_info(&selection.provider, &selection.model, None)
                            .await
                            .is_ok_and(|model| {
                                model.input_modalities.as_ref().is_some_and(|modalities| {
                                    !modalities.contains(&dsh_llm::ModelModality::Image)
                                })
                            }),
                        None => false,
                    }
                };
            if explicitly_rejects_images {
                return err(
                    request.rpc_id,
                    RpcError::AttachmentError(RpcErrorBody {
                        message: format!(
                            "model {}/{} does not declare image input support",
                            selection.provider, selection.model
                        ),
                        details: crate::api::rpc::ReasonDetails {
                            reason: "MODEL_DOES_NOT_SUPPORT_IMAGES".to_string(),
                        },
                    }),
                );
            }
        }
        let attachment_store = self
            .ctx
            .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
            .map(|slot| slot.as_ref().clone());
        let mut content = Vec::with_capacity(request.payload.content.len());
        for part in &request.payload.content {
            match part {
                PromptContentPart::Text { text } => {
                    content.push(dsh_llm::ContentBlock::Text { text: text.clone() });
                }
                PromptContentPart::Image {
                    media_type,
                    data,
                    name,
                } => {
                    let Some(store) = attachment_store.as_ref() else {
                        return err(
                            request.rpc_id,
                            RpcError::AttachmentError(RpcErrorBody {
                                message: "image input requires the attachments service".to_string(),
                                details: crate::api::rpc::ReasonDetails {
                                    reason: "ATTACHMENT_SERVICE_UNAVAILABLE".to_string(),
                                },
                            }),
                        );
                    };
                    use base64::Engine;
                    let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            return err(
                                request.rpc_id,
                                RpcError::AttachmentError(RpcErrorBody {
                                    message: format!("image data is not valid base64: {error}"),
                                    details: crate::api::rpc::ReasonDetails {
                                        reason: "INVALID_IMAGE".to_string(),
                                    },
                                }),
                            );
                        }
                    };
                    let saved = match store
                        .save_image(&dsh_attachment::SaveImageAttachment {
                            data: bytes,
                            media_type: *media_type,
                            name: name.clone(),
                        })
                        .await
                    {
                        Ok(reference) => reference,
                        Err(error) => {
                            return err(
                                request.rpc_id,
                                RpcError::AttachmentError(RpcErrorBody {
                                    message: error.to_string(),
                                    details: crate::api::rpc::ReasonDetails { reason: error.code },
                                }),
                            );
                        }
                    };
                    content.push(dsh_llm::ContentBlock::Image {
                        attachment: dsh_llm::ImageAttachmentRef {
                            attachment_id: saved.attachment_id.to_string(),
                            media_type: Some(saved.media_type.as_str().to_string()),
                            bytes: Some(saved.bytes),
                            width: Some(saved.width),
                            height: Some(saved.height),
                            name: saved.name,
                        },
                    });
                }
            }
        }
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
        self.spawn_idle_retirement(Arc::clone(&agent));
        drop(admission);
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
                    if object.get("type").and_then(serde_json::Value::as_str) == Some("image")
                        && let Some(reference) = object.get("attachment")
                        && reference
                            .get("attachmentId")
                            .and_then(serde_json::Value::as_str)
                            == Some(attachment_id)
                        && let Ok(reference) = serde_json::from_value::<
                            dsh_attachment::ImageAttachmentRef,
                        >(reference.clone())
                    {
                        return Some(reference);
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
        let reference = if let Some(session) =
            self.sessions().and_then(|store| store.get(&session_id))
        {
            let events = session.events();
            Self::referenced_image(events.as_slice(), &attachment_id)
        } else {
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
            let found = Arc::new(parking_lot::Mutex::new(None));
            let found_for_visitor = found.clone();
            let attachment_for_visitor = attachment_id.clone();
            let visitor: dsh_session_persistence::NonpackedEventVisitor = Arc::new(move |events| {
                if let Some(reference) = Self::referenced_image(events, &attachment_for_visitor) {
                    *found_for_visitor.lock() = Some(reference);
                    return Ok(false);
                }
                Ok(true)
            });
            if persistence
                .visit_nonpacked_events(&session_id, visitor)
                .await
                .is_err()
            {
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
            found.lock().clone()
        };
        let Some(reference) = reference else {
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
            if let Some(next) = &next_cursor
                && !seen_cursors.insert(next.to_string())
            {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: "session search provider repeated a continuation cursor"
                            .to_string(),
                        details: EmptyDetails {},
                    }),
                );
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
        let (page_events, has_more) = match Self::paginate(
            &events,
            request.payload.before_seq,
            request.payload.max_messages.unwrap_or(DEFAULT_MAX_MESSAGES),
        ) {
            Ok(page) => page,
            Err(required) => {
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!(
                            "subagent.history: one safe history group requires {required} events, above the 4096 event budget"
                        ),
                        details: EmptyDetails {},
                    }),
                );
            }
        };
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
        let attachment_store = self
            .ctx
            .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
            .map(|slot| slot.as_ref().clone());
        let image_count = request
            .payload
            .content
            .iter()
            .filter(|part| matches!(part, crate::api::sessions::PromptContentPart::Image { .. }))
            .count() as u64;
        let store = if image_count == 0 {
            None
        } else {
            let Some(store) = attachment_store.as_ref() else {
                return subagent_attachment_error(
                    request.rpc_id,
                    "subagent image input requires the attachments service",
                    "ATTACHMENT_SERVICE_UNAVAILABLE",
                );
            };
            let limits = store.image_limits();
            if image_count > limits.max_images_per_message {
                return subagent_attachment_error(
                    request.rpc_id,
                    "subagent image count exceeds the deployment limit",
                    "TOO_MANY_IMAGES",
                );
            }
            Some(store)
        };
        let limits = store.map(|store| store.image_limits());
        let mut pending_images = Vec::with_capacity(image_count as usize);
        let mut pending_parts = Vec::with_capacity(request.payload.content.len());
        let mut admission_content = Vec::with_capacity(request.payload.content.len());
        let mut message_image_bytes = 0_u64;
        for part in &request.payload.content {
            match part {
                crate::api::sessions::PromptContentPart::Text { text } => {
                    pending_parts.push(SubagentPromptPart::Text(text.clone()));
                    admission_content.push(dsh_llm::ContentBlock::Text { text: text.clone() });
                }
                crate::api::sessions::PromptContentPart::Image {
                    media_type,
                    data,
                    name,
                } => {
                    let limits = limits.expect("image count guarantees attachment limits");
                    let remaining_message_bytes = limits
                        .max_message_image_bytes
                        .saturating_sub(message_image_bytes);
                    let max_decoded_bytes = limits.max_image_bytes.min(remaining_message_bytes);
                    let max_encoded_bytes = max_decoded_bytes
                        .saturating_add(2)
                        .saturating_div(3)
                        .saturating_mul(4);
                    if data.len() as u64 > max_encoded_bytes {
                        let reason = if remaining_message_bytes < limits.max_image_bytes {
                            "MESSAGE_IMAGES_TOO_LARGE"
                        } else {
                            "IMAGE_TOO_LARGE"
                        };
                        return subagent_attachment_error(
                            request.rpc_id,
                            "subagent images exceed the deployment byte limit",
                            reason,
                        );
                    }
                    use base64::Engine;
                    let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            return subagent_attachment_error(
                                request.rpc_id,
                                format!("image data is not valid base64: {error}"),
                                "INVALID_IMAGE",
                            );
                        }
                    };
                    let decoded_bytes = bytes.len() as u64;
                    if decoded_bytes > limits.max_image_bytes {
                        return subagent_attachment_error(
                            request.rpc_id,
                            "subagent image exceeds the deployment byte limit",
                            "IMAGE_TOO_LARGE",
                        );
                    }
                    message_image_bytes = match message_image_bytes.checked_add(decoded_bytes) {
                        Some(total) if total <= limits.max_message_image_bytes => total,
                        _ => {
                            return subagent_attachment_error(
                                request.rpc_id,
                                "subagent images exceed the aggregate deployment byte limit",
                                "MESSAGE_IMAGES_TOO_LARGE",
                            );
                        }
                    };
                    let image_index = pending_images.len();
                    pending_images.push(dsh_attachment::SaveImageAttachment {
                        data: bytes,
                        media_type: *media_type,
                        name: name.clone(),
                    });
                    pending_parts.push(SubagentPromptPart::Image(image_index));
                    admission_content.push(dsh_llm::ContentBlock::Image {
                        attachment: dsh_llm::ImageAttachmentRef {
                            attachment_id: format!("pending:{image_index}"),
                            media_type: Some(media_type.as_str().to_string()),
                            bytes: Some(decoded_bytes),
                            width: None,
                            height: None,
                            name: name.clone(),
                        },
                    });
                }
            }
        }
        let source = dsh_llm::MessageSource::User {
            rpc_id: Some(request.rpc_id.to_string()),
            client_time_zone: canonical_time_zone,
        };
        let abort_flag = signal.clone();
        let options = dsh_subagent::SubagentFollowupOptions {
            source,
            signal: Arc::new(move || abort_flag.aborted()),
        };
        if pending_images.is_empty() {
            return match runtime
                .followup(parent, &child_id, &admission_content, options)
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
            };
        }
        let admission = match runtime
            .admit_followup(parent, &child_id, &admission_content, options)
            .await
        {
            Ok(admission) => admission,
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
                if error.code == "MODEL_DOES_NOT_SUPPORT_IMAGES" {
                    return err(
                        request.rpc_id,
                        RpcError::AttachmentError(RpcErrorBody {
                            message: error.to_string(),
                            details: crate::api::rpc::ReasonDetails { reason: error.code },
                        }),
                    );
                }
                return err(
                    request.rpc_id,
                    RpcError::Internal(RpcErrorBody {
                        message: format!("subagent prompt failed: {error}"),
                        details: EmptyDetails {},
                    }),
                );
            }
        };
        let saved_images = if pending_images.is_empty() {
            Vec::new()
        } else {
            let store = store.expect("pending images guarantee attachment store");
            match store.save_images(&pending_images).await {
                Ok(references) => references,
                Err(error) => {
                    runtime.abort_followup(admission).await;
                    return subagent_attachment_error(
                        request.rpc_id,
                        error.to_string(),
                        error.code,
                    );
                }
            }
        };
        let mut content = Vec::with_capacity(pending_parts.len());
        for part in pending_parts {
            match part {
                SubagentPromptPart::Text(text) => {
                    content.push(dsh_llm::ContentBlock::Text { text });
                }
                SubagentPromptPart::Image(index) => {
                    let saved = &saved_images[index];
                    content.push(dsh_llm::ContentBlock::Image {
                        attachment: dsh_llm::ImageAttachmentRef {
                            attachment_id: saved.attachment_id.to_string(),
                            media_type: Some(saved.media_type.as_str().to_string()),
                            bytes: Some(saved.bytes),
                            width: Some(saved.width),
                            height: Some(saved.height),
                            name: saved.name.clone(),
                        },
                    });
                }
            }
        }
        let message_id = runtime.submit_followup(admission, &content);
        ok(request.rpc_id, SubagentPromptReceipt { message_id })
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
                let recompose_ctx = agent_for_swap.ctx().clone();
                let recompose_presets = presets.clone();
                let recompose_id = agent_preset_for_swap.clone();
                let session_for_commit = session_for_swap.clone();
                let switched = tokio::task::spawn_blocking(move || {
                    let preset = futures::executor::block_on(
                        recompose_presets.recompose(&recompose_ctx, &recompose_id),
                    )?;
                    session_for_commit
                        .append(
                            dsh_agent_presets::AGENT_PRESET_SELECTED,
                            dsh_agent_presets::selected_data(&preset.id),
                            None,
                        )
                        .map_err(|error| dsh_agent_presets::PresetMountError {
                            preset_id: preset.id.clone(),
                            reason: format!("selection log append failed: {error}"),
                        })?;
                    Ok::<_, dsh_agent_presets::PresetMountError>(preset)
                })
                .await;
                let switched = match switched {
                    Ok(result) => result,
                    Err(error) => {
                        return Arc::new(err(
                            rpc_id_for_swap,
                            RpcError::Internal(RpcErrorBody {
                                message: format!(
                                    "agent preset switch worker failed for \"{agent_preset_for_swap}\": {error}"
                                ),
                                details: EmptyDetails {},
                            }),
                        ));
                    }
                };
                match switched {
                    Ok(preset) => Arc::new(ok(
                        rpc_id_for_swap,
                        crate::api::agent_presets::AgentPresetSelectResult {
                            agent_preset: preset.id,
                        },
                    )),
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
            "messageFeedback.list" => {
                let payload: dsh_message_feedback::MessageFeedbackListRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("messageFeedback.list", error));
                        }
                    };
                let Some(service) = self
                    .ctx
                    .get_typed::<Arc<dsh_message_feedback::MessageFeedbackService>>(
                        "messageFeedback",
                        false,
                    )
                    .map(|slot| slot.as_ref().clone())
                else {
                    return err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: "message feedback service is unavailable".to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                };
                match service.list(&payload).await {
                    Ok(value) => ok(rpc_id, value),
                    Err(value) => ok(rpc_id, value),
                }
            }
            "messageFeedback.put" => {
                let payload: dsh_message_feedback::MessageFeedbackPutRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("messageFeedback.put", error));
                        }
                    };
                let Some(service) = self
                    .ctx
                    .get_typed::<Arc<dsh_message_feedback::MessageFeedbackService>>(
                        "messageFeedback",
                        false,
                    )
                    .map(|slot| slot.as_ref().clone())
                else {
                    return err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: "message feedback service is unavailable".to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                };
                match service.put(&payload).await {
                    Ok(value) => ok(rpc_id, value),
                    Err(value) => ok(rpc_id, value),
                }
            }
            "messageFeedback.delete" => {
                let payload: dsh_message_feedback::MessageFeedbackDeleteRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("messageFeedback.delete", error));
                        }
                    };
                let Some(service) = self
                    .ctx
                    .get_typed::<Arc<dsh_message_feedback::MessageFeedbackService>>(
                        "messageFeedback",
                        false,
                    )
                    .map(|slot| slot.as_ref().clone())
                else {
                    return err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: "message feedback service is unavailable".to_string(),
                            details: EmptyDetails {},
                        }),
                    );
                };
                match service.delete(&payload).await {
                    Ok(value) => ok(rpc_id, value),
                    Err(value) => ok(rpc_id, value),
                }
            }
            "pluginInventory.list" => {
                self.plugin_inventory_list(RpcRequest {
                    rpc_id,
                    payload: serde_json::Value::Null,
                })
                .await
            }
            "pluginInventory.setEnabled" => {
                let payload: dsh_host_plugin_inventory::PluginSetEnabledRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("pluginInventory.setEnabled", error));
                        }
                    };
                self.plugin_inventory_set_enabled(RpcRequest { rpc_id, payload })
                    .await
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
            "commands.execute" => {
                let args = request.payload.get("args");
                let session_id = args
                    .and_then(|args| args.get("agentId"))
                    .and_then(serde_json::Value::as_str)
                    .map(dsh_session::session_id);
                let line = args
                    .and_then(|args| args.get("line"))
                    .and_then(serde_json::Value::as_str);
                let (Some(session_id), Some(line)) = (session_id, line) else {
                    return err(
                        rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: "commands.execute requires args.agentId and args.line"
                                .to_string(),
                            details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                        }),
                    );
                };
                let agent = match self.resolver.resolve(&session_id).await {
                    crate::agent_lookup::ApiRemoteAgentResult::Agent(agent) => agent,
                    crate::agent_lookup::ApiRemoteAgentResult::Error(error) => {
                        return err(rpc_id, error);
                    }
                };
                let Some(commands) = self
                    .ctx
                    .get_typed::<Arc<dsh_commands::CommandRuntime>>("commands", false)
                    .map(|slot| slot.as_ref().clone())
                else {
                    return ok(rpc_id, serde_json::Value::Null);
                };
                let command_signal = signal.clone();
                let abort = Arc::new(move || command_signal.aborted());
                match commands.execute(&agent, line, abort).await {
                    Ok(Some(execution)) => ok(
                        rpc_id,
                        serde_json::json!({
                            "commandId": execution.command_id.as_str(),
                            "result": execution.result,
                        }),
                    ),
                    Ok(None) => ok(rpc_id, serde_json::Value::Null),
                    Err(error) if signal.aborted() => err(
                        rpc_id,
                        RpcError::Cancelled(RpcErrorBody {
                            message: error,
                            details: EmptyDetails {},
                        }),
                    ),
                    Err(error) => err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: error,
                            details: EmptyDetails {},
                        }),
                    ),
                }
            }
            "commands.list" => {
                let session_id = request
                    .payload
                    .get("args")
                    .and_then(|args| args.get("agentId"))
                    .and_then(serde_json::Value::as_str)
                    .map(dsh_session::session_id);
                let Some(session_id) = session_id else {
                    return err(
                        rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: "commands.list requires args.agentId".to_string(),
                            details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                        }),
                    );
                };
                let Some(commands) = self
                    .ctx
                    .get_typed::<Arc<dsh_commands::CommandRuntime>>("commands", false)
                    .map(|slot| slot.as_ref().clone())
                else {
                    return ok(rpc_id, serde_json::json!([]));
                };
                let descriptors = if let Some(agent) =
                    self.agents().and_then(|agents| agents.get(&session_id))
                {
                    commands.list(&agent)
                } else {
                    let Some(persistence) = self
                        .ctx
                        .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                            "sessionPersistence",
                            false,
                        )
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return err(
                            rpc_id,
                            RpcError::Internal(RpcErrorBody {
                                message: "commands.list: session persistence is not composed"
                                    .to_string(),
                                details: EmptyDetails {},
                            }),
                        );
                    };
                    let headers = match persistence.list().await {
                        Ok(headers) => headers,
                        Err(error) => {
                            return err(
                                rpc_id,
                                RpcError::Internal(RpcErrorBody {
                                    message: format!(
                                        "commands.list: persistence unavailable: {error}"
                                    ),
                                    details: EmptyDetails {},
                                }),
                            );
                        }
                    };
                    let Some(header) = headers.into_iter().find(|header| header.id == session_id)
                    else {
                        return err(
                            rpc_id,
                            RpcError::SessionNotFound(RpcErrorBody {
                                message: format!("session \"{session_id}\" not found"),
                                details: crate::api::rpc::SessionIdDetails {
                                    session_id: session_id.to_string(),
                                },
                            }),
                        );
                    };
                    let Some(presets) = self.agent_presets() else {
                        return ok(rpc_id, serde_json::json!([]));
                    };
                    let scope = match presets
                        .standing_key_for(header.agent_preset.as_deref())
                        .await
                    {
                        Ok(scope) => scope,
                        Err(error) => {
                            return err(
                                rpc_id,
                                RpcError::Internal(RpcErrorBody {
                                    message: format!(
                                        "commands.list: preset standing scope unavailable: {error}"
                                    ),
                                    details: EmptyDetails {},
                                }),
                            );
                        }
                    };
                    commands.list_for_scope(Some(&scope))
                };
                ok(
                    rpc_id,
                    serde_json::to_value(descriptors).expect("commands serialize"),
                )
            }
            "settings.describe" => {
                self.settings_describe(RpcRequest {
                    rpc_id,
                    payload: request.payload,
                })
                .await
            }
            "capabilities.list"
            | "capabilities.skillRead"
            | "capabilities.skillSave"
            | "capabilities.skillRemove"
            | "capabilities.skillToggle"
            | "capabilities.serverSave"
            | "capabilities.serverToggle"
            | "capabilities.serverRemove"
            | "capabilities.serverTest" => {
                let Some(manager) = self
                    .ctx
                    .get_typed::<Arc<crate::capabilities::CapabilityManager>>(
                        "capabilityManager",
                        false,
                    )
                else {
                    return err(
                        rpc_id,
                        RpcError::Internal(RpcErrorBody {
                            message: "capability manager unavailable".into(),
                            details: EmptyDetails {},
                        }),
                    );
                };
                match manager.invoke(method, request.payload).await {
                    Ok(value) => ok(rpc_id, value),
                    Err(error) => err(
                        rpc_id,
                        RpcError::BadRequest(RpcErrorBody {
                            message: error,
                            details: crate::api::rpc::BadRequestDetails { issues: vec![] },
                        }),
                    ),
                }
            }
            "memory.categories" | "memory.list" | "memory.upsert" | "memory.remove" => {
                self.memory_rpc(rpc_id, method, request.payload).await
            }
            "memory.learningList"
            | "memory.learningConfigure"
            | "memory.learningToggle"
            | "memory.learningRemove"
            | "memory.learningConfirm"
            | "memory.learningPreview" => self.learning_rpc(rpc_id, method, request.payload).await,
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
            "workspace.deleteArchivedSession" => {
                let payload: crate::api::workspace::WorkspaceArchiveSessionRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(
                                rpc_id,
                                bad_request("workspace.deleteArchivedSession", error),
                            );
                        }
                    };
                self.workspace_delete_archived_session(RpcRequest { rpc_id, payload })
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
            "session.updateTodos" => {
                let payload: crate::api::sessions::SessionUpdateTodosRequest =
                    match serde_json::from_value(request.payload) {
                        Ok(payload) => payload,
                        Err(error) => {
                            return err(rpc_id, bad_request("session.updateTodos", error));
                        }
                    };
                self.session_update_todos(RpcRequest { rpc_id, payload })
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
                    "lastSeq": session.seq().get() as i64 - 1,
                }),
            });
        }
        // Register after the session baseline; subscribe() atomically inserts
        // the queue and replays every still-pending interaction, so a request
        // created during baseline construction is retained rather than lost.
        let subscription = self.interactions.subscribe(tx.clone());
        // Live session events ride the global cordis stream with the same
        // tool presentation intent cold history derives.
        let tx_for_listener = tx.clone();
        let tools_for_listener = self
            .ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone());
        let agents_for_listener = self.agents();
        let listener: Arc<cordis::Listener> = Arc::new(
            move |_dispatch_ctx: &Context, args: Vec<cordis::ArcValue>| {
                let tx = tx_for_listener.clone();
                let tools = tools_for_listener.clone();
                let agents = agents_for_listener.clone();
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
                        let view = if event.type_ == "tool/call" {
                            let name = event.data.get("name").and_then(serde_json::Value::as_str);
                            let arguments = event
                                .data
                                .get("arguments")
                                .and_then(serde_json::Value::as_str)
                                .and_then(|value| {
                                    serde_json::from_str::<serde_json::Value>(value).ok()
                                });
                            let scope = agents
                                .as_ref()
                                .and_then(|agents| agents.get(session.id()))
                                .map(|agent| agent.scope_key().clone());
                            match (tools.as_ref(), name, arguments.as_ref()) {
                                (Some(tools), Some(name), Some(arguments)) => tools
                                    .present_call_for_scope(scope.as_ref(), name, arguments)
                                    .map(|view| crate::api::events::ToolEventView::Call { view }),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        let _ = tx.send(FrameRequest {
                            rpc_id: crate::api::rpc::rpc_id(Self::fresh_id()),
                            payload: serde_json::to_value(
                                crate::api::events::MuxFrame::SessionEventFrame {
                                    session_id: session.id().clone(),
                                    event,
                                    view,
                                },
                            )
                            .expect("session/event mux frame serialization"),
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
        let mut listener_disposers = vec![listener_disposer];
        if let Some(projections) = self
            .ctx
            .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                "sessionProjections",
                false,
            )
            .map(|slot| slot.as_ref().clone())
        {
            let tx_for_projection = tx.clone();
            let projection_listener: dsh_session_projection::ProjectionChangeListener =
                Arc::new(move |session, key, value, seq| {
                    let _ = tx_for_projection.send(FrameRequest {
                        rpc_id: crate::api::rpc::rpc_id(Self::fresh_id()),
                        payload: serde_json::to_value(
                            crate::api::events::MuxFrame::SessionProjection {
                                session_id: session.id().clone(),
                                key: key.to_string(),
                                value: value.clone(),
                                seq,
                            },
                        )
                        .expect("session/projection mux frame serialization"),
                    });
                });
            let projection_disposer = projections.on_changed(&self.ctx, projection_listener);
            self.ctx.fiber.disposables.delete(&projection_disposer);
            listener_disposers.push(projection_disposer);
        }
        let resources = crate::interactions::MuxResources::new(subscription, listener_disposers);
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

        let setup = async move {
            let mut listener_disposers = Vec::new();
            // session/created → host/session-added.
            let tx_created = tx.clone();
            let d_created = ctx
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
            listener_disposers.push(d_created);
            // session/disposed retires only the live runtime. The durable
            // session remains in persistence and must stay visible in the
            // browser list; permanent deletion has its own workspace event.
            let tx_disposed = tx.clone();
            let d_disposed = ctx
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
                                    crate::api::events::HostFrame::SessionStatus {
                                        session_id: session.id().clone(),
                                        running: false,
                                    },
                                );
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            listener_disposers.push(d_disposed);
            // workspace/session-deleted → host/session-removed.
            let tx_session_deleted = tx.clone();
            let d_session_deleted = ctx
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
            listener_disposers.push(d_session_deleted);
            // agent/status → host/session-status. The Agent Loop already
            // publishes exact Running/Idle transitions; forwarding them is
            // what clears the browser's busy state after turn/end.
            let tx_status = tx.clone();
            let d_status = ctx
                .on(
                    "agent/status",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_status.clone();
                        Box::pin(async move {
                            if let Some(payload) = args
                                .first()
                                .and_then(|value| {
                                    cordis::downcast::<dsh_agent::AgentStatusPayload>(value)
                                })
                                .cloned()
                            {
                                push(
                                    &tx,
                                    crate::api::events::HostFrame::SessionStatus {
                                        session_id: payload.agent.id().clone(),
                                        running: payload.status == dsh_agent::AgentStatus::Running,
                                    },
                                );
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default().global(true),
                )
                .await;
            listener_disposers.push(d_status);
            // domain/changed → the workspace frame family. A committed
            // workspace id the registry cannot resolve is skipped instead
            // of throwing (the Rust listener has no throw path).
            let tx_domain = tx.clone();
            let domain_ids = committed_ids.clone();
            let domain_order = committed_order.clone();
            let domain_archived = archived_ids.clone();
            let domain_registry = workspace_registry.clone();
            let d_domain = ctx
                .on(
                    "domain/changed",
                    Arc::new(move |_dispatch_ctx, args| {
                        let tx = tx_domain.clone();
                        let committed_ids = domain_ids.clone();
                        let committed_order = domain_order.clone();
                        let archived_ids = domain_archived.clone();
                        let registry = domain_registry.clone();
                        Box::pin(async move {
                            let change = args
                                .first()
                                .and_then(|value| {
                                    cordis::downcast::<dsh_storage_domain::DomainChanged>(value)
                                })
                                .cloned()?;
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
            listener_disposers.push(d_domain);
            // Allowlisted host events ride one verbatim wrapper frame each.
            for name in REMOTE_FORWARDED {
                let tx_remote = tx.clone();
                let d_remote = ctx
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
                listener_disposers.push(d_remote);
            }
            listener_disposers
        };
        let listener_disposers = futures::executor::block_on(setup);
        let _ = request;
        let stream_signal = signal.clone();
        let resources = crate::interactions::MuxResources::listeners(listener_disposers);
        let stream = futures::stream::unfold((rx, resources), move |(mut rx, resources)| {
            let signal = stream_signal.clone();
            async move {
                if signal.aborted() {
                    return None;
                }
                tokio::select! {
                    biased;
                    _ = signal.cancelled() => None,
                    frame = rx.recv() => frame.map(|frame| (frame, (rx, resources))),
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
            SessionLogExportDeps, flush_live_session_log, produce_session_log_zip_entries,
            session_log_zip_filename, stream_session_log_zip,
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
                body: Some(crate::fetch::handler::Body::Bytes(
                    b"session log export is unavailable: missing session-query, session-persistence, or attachments service"
                        .to_vec(),
                )),
            };
        }
        let persistence = deps.session_persistence.as_ref().expect("checked");
        if !persistence.supports_raw_artifacts() {
            return DownloadResponse {
                status: http::StatusCode::NOT_IMPLEMENTED,
                headers: Vec::new(),
                body: Some(crate::fetch::handler::Body::Bytes(
                    b"session log export is unavailable: the persistence backend does not expose per-session raw artifacts"
                        .to_vec(),
                )),
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
                body: Some(crate::fetch::handler::Body::Bytes(
                    b"session log export failed to prepare the stored artifact".to_vec(),
                )),
            };
        }
        let root = match persistence.read_raw(&session_id).await {
            Ok(root) => root,
            Err(_) => {
                return DownloadResponse {
                    status: http::StatusCode::INTERNAL_SERVER_ERROR,
                    headers: Vec::new(),
                    body: Some(crate::fetch::handler::Body::Bytes(
                        b"session log export failed to prepare the stored artifact".to_vec(),
                    )),
                };
            }
        };
        let Some(root) = root else {
            return DownloadResponse {
                status: http::StatusCode::NOT_FOUND,
                headers: Vec::new(),
                body: Some(crate::fetch::handler::Body::Bytes(
                    b"session not found".to_vec(),
                )),
            };
        };
        let filename = session_log_zip_filename(&session_id);
        let (entry_sender, entry_receiver) = tokio::sync::mpsc::channel(1);
        let producer_signal = signal.clone();
        let producer_id = session_id.clone();
        tokio::spawn(async move {
            if let Err(error) = produce_session_log_zip_entries(
                &deps,
                root,
                &producer_id,
                query.include_descendants.unwrap_or(false),
                &producer_signal,
                &entry_sender,
            )
            .await
            {
                let _ = entry_sender.send(Err(error)).await;
            }
        });
        let body = crate::fetch::handler::Body::Stream(stream_session_log_zip(
            entry_receiver,
            self.defaults.session_export_compression_level.min(9) as u8,
            signal,
        ));
        DownloadResponse {
            status: http::StatusCode::OK,
            headers: vec![
                ("content-type".to_string(), "application/zip".to_string()),
                (
                    "content-disposition".to_string(),
                    format!("attachment; filename=\"{}\"", filename),
                ),
            ],
            body: Some(body),
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
