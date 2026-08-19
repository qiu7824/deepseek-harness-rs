//! Host BFF policy for resolving Remote Agent and Session identities.
//! Rust port of `packages/api/remotes/src/agent-lookup.ts`.
//!
//! # Deviations
//!
//! - The TS `persistence.list()` existence pre-check collapses into the
//!   Rust coordinator's `inspect` (an absent id fails the same way); the
//!   `meta.cwd` check keeps the TS session-not-found semantics.
//! - Single-flight resumes share one `futures::future::Shared` per identity
//!   (the TS `resumes` map holds one promise per identity); the entry is
//!   removed when the shared future settles, so a failed resume retries on
//!   the next call exactly like TS.
//! - The typert lookup configuration is absent until the typert milestone
//!   (same deferral as the goal crate's remote units, round 49).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentRegistry, AgentSetup, ResumeAgentOptions};
use dsh_session::{SessionEvent, SessionHeader, SessionId};
use dsh_session_persistence::SessionPersistenceApi;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;

use crate::api::rpc::{EmptyDetails, ReasonDetails, RpcError, RpcErrorBody, SessionIdDetails};

/// Resume configuration supplied by the owning Host composition.
pub struct ApiRemoteAgentOptions {
    /// Read the per-Agent defaults when a cold identity must resume.
    pub agent_options: Arc<dyn Fn() -> dsh_agent::AgentOptions + Send + Sync>,
    /// Build the Host-specific Agent-scope composition completed before
    /// publication, keyed by the resumed session itself.
    pub setup: Option<
        Arc<
            dyn Fn(
                    SessionHeader,
                    Vec<SessionEvent>,
                ) -> BoxFuture<'static, Result<Option<AgentSetup>, String>>
                + Send
                + Sync,
        >,
    >,
}

/// The single-flight resume failure classification (caller-facing errors
/// are [`RpcError`]s; this is the internal channel).
#[derive(Clone)]
enum ResumeFailure {
    SessionNotFound(String),
    SubagentOwned(SessionId),
    Internal(String),
}

type SharedResume = Shared<BoxFuture<'static, Result<Arc<dyn Agent>, ResumeFailure>>>;

/// Result of resolving one session identity to its live Agent.
pub enum ApiRemoteAgentResult {
    Agent(Arc<dyn Agent>),
    Error(RpcError),
}

impl std::fmt::Debug for ApiRemoteAgentResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent(agent) => write!(f, "Agent({})", agent.id()),
            Self::Error(error) => write!(f, "Error({})", error.code().as_str()),
        }
    }
}

/// Test whether generic Host routing must leave an identity to subagent
/// routing (TS `hasApiRemoteSubagentOwner`).
pub fn has_api_remote_subagent_owner(
    ctx: &Context,
    header: &SessionHeader,
    agent: Option<&Arc<dyn Agent>>,
) -> bool {
    if header.origin.as_deref() == Some("subagent") {
        return true;
    }
    let Some(parent_id) = &header.parent_session else {
        return false;
    };
    let Some(agent) = agent else {
        return false;
    };
    let Some(registry) = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .map(|slot| slot.as_ref().clone())
    else {
        return false;
    };
    let Some(parent) = registry.get(parent_id) else {
        return false;
    };
    registry.is_owned_by(agent.id(), &parent)
}

/// The stable caller-facing ownership rejection.
fn subagent_ownership_error(session_id: &SessionId) -> RpcError {
    RpcError::AgentBusy(RpcErrorBody {
        message: format!("session \"{session_id}\" is owned by subagent routing"),
        details: ReasonDetails {
            reason: "use subagent delivery for this child session".to_string(),
        },
    })
}

/// The Host's shared Agent resolver: live Agents are reused, ordinary cold
/// sessions resume once per identity, and subagent-owned identities retain
/// the `agent-busy` fence.
pub struct AgentResolver {
    ctx: Context,
    options: Arc<ApiRemoteAgentOptions>,
    resumes: Mutex<HashMap<SessionId, SharedResume>>,
}

impl AgentResolver {
    pub fn new(ctx: &Context, options: ApiRemoteAgentOptions) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            options: Arc::new(options),
            resumes: Mutex::new(HashMap::new()),
        })
    }

    fn agents(&self) -> Option<Arc<AgentRegistry>> {
        self.ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
    }

    fn sessions(&self) -> Option<Arc<dsh_session::SessionStore>> {
        self.ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
    }

    /// Live Agents pass through with the ownership fence applied.
    fn fenced_live_agent(&self, session_id: &SessionId) -> Option<ApiRemoteAgentResult> {
        let live = self.agents()?.get(session_id)?;
        if has_api_remote_subagent_owner(self.ctx(), live.session().header(), Some(&live)) {
            Some(ApiRemoteAgentResult::Error(subagent_ownership_error(
                session_id,
            )))
        } else {
            Some(ApiRemoteAgentResult::Agent(live))
        }
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    /// Resolve one session identity to its live Agent (live reuse, single
    /// -flight cold resume, ownership fences).
    pub async fn resolve(&self, session_id: &SessionId) -> ApiRemoteAgentResult {
        if let Some(fenced) = self.fenced_live_agent(session_id) {
            return fenced;
        }
        if let Some(sessions) = self.sessions() {
            if let Some(attached) = sessions.get(session_id) {
                if has_api_remote_subagent_owner(&self.ctx, attached.header(), None) {
                    return ApiRemoteAgentResult::Error(subagent_ownership_error(session_id));
                }
            }
        }

        let shared = {
            let mut resumes = self.resumes.lock();
            if let Some(shared) = resumes.get(session_id) {
                shared.clone()
            } else {
                let resume_id = session_id.clone();
                let ctx = self.ctx.clone();
                let options = self.options.clone();
                let future: BoxFuture<'static, Result<Arc<dyn Agent>, ResumeFailure>> =
                    Box::pin(async move {
                        let (meta, events) = inspect_cold(&ctx, &resume_id).await?;
                        if has_api_remote_subagent_owner(&ctx, &meta, None) {
                            return Err(ResumeFailure::SubagentOwned(resume_id.clone()));
                        }
                        let setup = match &options.setup {
                            None => None,
                            Some(build) => build(meta.clone(), events.clone())
                                .await
                                .map_err(|error| ResumeFailure::Internal(error))?,
                        };
                        // Re-check published state before resuming (the TS
                        // collision-window guard).
                        let published_owned = ctx
                            .get_typed::<Arc<AgentRegistry>>("agents", false)
                            .and_then(|slot| {
                                let registry = slot.as_ref().clone();
                                registry.get(&resume_id).map(|agent| {
                                    has_api_remote_subagent_owner(
                                        &ctx,
                                        agent.session().header(),
                                        Some(&agent),
                                    )
                                })
                            })
                            .unwrap_or(false)
                            || ctx
                                .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                                .and_then(|slot| {
                                    slot.as_ref().clone().get(&resume_id).map(|attached| {
                                        has_api_remote_subagent_owner(&ctx, attached.header(), None)
                                    })
                                })
                                .unwrap_or(false);
                        if published_owned {
                            return Err(ResumeFailure::SubagentOwned(resume_id.clone()));
                        }
                        let Some(registry) = ctx
                            .get_typed::<Arc<AgentRegistry>>("agents", false)
                            .map(|slot| slot.as_ref().clone())
                        else {
                            return Err(ResumeFailure::Internal(
                                "the agents service is not composed".to_string(),
                            ));
                        };
                        let options_builder = ResumeAgentOptions {
                            resume_session_id: Some(resume_id),
                            agent_options: Some((options.agent_options)()),
                            setup,
                        };
                        let handle = registry
                            .resume(options_builder)
                            .await
                            .map_err(|error| ResumeFailure::Internal(error))?;
                        Ok(handle.agent)
                    });
                let shared: SharedResume = future.shared();
                resumes.insert(session_id.clone(), shared.clone());
                shared
            }
        };

        match shared.await {
            Ok(agent) => ApiRemoteAgentResult::Agent(agent),
            Err(failure) => {
                // Remove the settled entry so a failed resume retries next
                // call (the TS finally-delete semantics).
                self.resumes.lock().remove(session_id);
                match failure {
                    ResumeFailure::SessionNotFound(message) => {
                        ApiRemoteAgentResult::Error(RpcError::SessionNotFound(RpcErrorBody {
                            message,
                            details: SessionIdDetails {
                                session_id: session_id.to_string(),
                            },
                        }))
                    }
                    ResumeFailure::SubagentOwned(id) => {
                        ApiRemoteAgentResult::Error(subagent_ownership_error(&id))
                    }
                    ResumeFailure::Internal(message) => {
                        // Last-chance live checks before the internal report.
                        if let Some(fenced) = self.fenced_live_agent(session_id) {
                            return fenced;
                        }
                        ApiRemoteAgentResult::Error(RpcError::Internal(RpcErrorBody {
                            message: format!(
                                "resume failed for session \"{session_id}\": {message}"
                            ),
                            details: EmptyDetails {},
                        }))
                    }
                }
            }
        }
    }
}

/// The cold-inspection step (free function so the single-flight closure
/// stays testable and the resolver stays clonable).
async fn inspect_cold(
    ctx: &Context,
    session_id: &SessionId,
) -> Result<(SessionHeader, Vec<SessionEvent>), ResumeFailure> {
    let Some(persistence) = ctx
        .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
        .map(|slot| slot.as_ref().clone())
    else {
        return Err(ResumeFailure::Internal(
            "session persistence is not configured (load a dsh-session-persistence backend)"
                .to_string(),
        ));
    };
    let inspected = persistence.inspect(session_id).await.map_err(|_| {
        ResumeFailure::SessionNotFound(format!("session \"{session_id}\" not found"))
    })?;
    if inspected.meta.cwd.is_none() {
        return Err(ResumeFailure::SessionNotFound(format!(
            "session \"{session_id}\" not found"
        )));
    }
    Ok((inspected.meta, inspected.events))
}
