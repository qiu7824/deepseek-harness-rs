//! Host BFF policy for resolving Remote Agent and Session identities.
//! Rust port of `packages/api/remotes/src/agent-lookup.ts`.
//!
//! # Deviations
//!
//! - Cold routing reads the authoritative header only. Full validation and
//!   repair belong to the loop's one reserved preparation; setup receives
//!   that exact unpublished Session instead of a second transcript copy.
//! - Single-flight resumes share one `futures::future::Shared` per identity
//!   (the TS `resumes` map holds one promise per identity); the entry is
//!   removed when the shared future settles or its last caller cancels, so
//!   failures retry and abandoned unpublished resources can roll back.
//! - The typert lookup configuration is absent until the typert milestone
//!   (same deferral as the goal crate's remote units, round 49).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentRegistry, AgentSetup, ResumeAgentOptions};
use dsh_session::{SessionHeader, SessionId};
use dsh_session_persistence::SessionPersistenceApi;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;

use crate::api::rpc::{EmptyDetails, ReasonDetails, RpcError, RpcErrorBody, SessionIdDetails};

/// Resume configuration supplied by the owning Host composition.
pub type ApiRemoteAgentSetup = AgentSetup;

pub struct ApiRemoteAgentOptions {
    /// Read the per-Agent defaults when a cold identity must resume.
    pub agent_options: Arc<dyn Fn() -> dsh_agent::AgentOptions + Send + Sync>,
    /// Retain the owner-only lifecycle handle for an API-resumed Agent.
    pub retain_handle: Arc<dyn Fn(dsh_agent::AgentHandle) -> Arc<dyn Agent> + Send + Sync>,
    /// Build the Host-specific Agent-scope composition completed before
    /// publication, keyed by the resumed session itself.
    pub setup: Option<ApiRemoteAgentSetup>,
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

struct ResumeEntry {
    future: SharedResume,
    waiters: usize,
}

/// The map coordinates current callers, rather than owning an abandoned
/// preparation forever when the last caller cancels during async setup.
struct ResumeWaiter<'a> {
    resumes: &'a Mutex<HashMap<SessionId, ResumeEntry>>,
    id: SessionId,
    future: SharedResume,
}

impl Drop for ResumeWaiter<'_> {
    fn drop(&mut self) {
        let removed = {
            let mut resumes = self.resumes.lock();
            if let Some(entry) = resumes.get_mut(&self.id)
                && entry.future.ptr_eq(&self.future)
            {
                entry.waiters -= 1;
                if entry.waiters == 0 {
                    resumes.remove(&self.id)
                } else {
                    None
                }
            } else {
                None
            }
        };
        drop(removed);
    }
}

#[derive(Default)]
struct RetirementGates {
    inner: Arc<Mutex<HashMap<SessionId, RetirementGate>>>,
    next_token: std::sync::atomic::AtomicU64,
}

struct RetirementGate {
    token: u64,
    done: tokio::sync::watch::Receiver<bool>,
    publish_done: tokio::sync::watch::Sender<bool>,
}

pub(crate) struct RetirementGuard {
    session_id: SessionId,
    token: u64,
    inner: Arc<Mutex<HashMap<SessionId, RetirementGate>>>,
}

impl Drop for RetirementGuard {
    fn drop(&mut self) {
        let publish = {
            let mut inner = self.inner.lock();
            let is_current = inner
                .get(&self.session_id)
                .is_some_and(|gate| gate.token == self.token);
            if !is_current {
                return;
            }
            inner.remove(&self.session_id).map(|gate| gate.publish_done)
        };
        if let Some(publish) = publish {
            let _ = publish.send(true);
        }
    }
}

impl RetirementGates {
    fn begin(&self, session_id: &SessionId) -> RetirementGuard {
        let token = self
            .next_token
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let (publish_done, done) = tokio::sync::watch::channel(false);
        self.inner.lock().insert(
            session_id.clone(),
            RetirementGate {
                token,
                done,
                publish_done,
            },
        );
        RetirementGuard {
            session_id: session_id.clone(),
            token,
            inner: Arc::clone(&self.inner),
        }
    }

    async fn wait(&self, session_id: &SessionId) {
        loop {
            let receiver = self
                .inner
                .lock()
                .get(session_id)
                .map(|gate| gate.done.clone());
            let Some(mut receiver) = receiver else {
                return;
            };
            while !*receiver.borrow() {
                if receiver.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}

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
    resumes: Mutex<HashMap<SessionId, ResumeEntry>>,
    retirements: RetirementGates,
    admissions: Mutex<HashMap<SessionId, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

impl AgentResolver {
    pub fn new(ctx: &Context, options: ApiRemoteAgentOptions) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            options: Arc::new(options),
            resumes: Mutex::new(HashMap::new()),
            retirements: RetirementGates::default(),
            admissions: Mutex::new(HashMap::new()),
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

    pub(crate) fn admission(&self, session_id: &SessionId) -> Arc<tokio::sync::Mutex<()>> {
        let mut admissions = self.admissions.lock();
        admissions.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = admissions
            .get(session_id)
            .and_then(std::sync::Weak::upgrade)
        {
            return gate;
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        admissions.insert(session_id.clone(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) fn begin_retirement(&self, session_id: &SessionId) -> RetirementGuard {
        self.retirements.begin(session_id)
    }

    /// Resolve one session identity to its live Agent (live reuse, single
    /// -flight cold resume, ownership fences).
    pub async fn resolve(&self, session_id: &SessionId) -> ApiRemoteAgentResult {
        self.retirements.wait(session_id).await;
        if let Some(fenced) = self.fenced_live_agent(session_id) {
            return fenced;
        }
        if let Some(sessions) = self.sessions()
            && let Some(attached) = sessions.get(session_id)
            && has_api_remote_subagent_owner(&self.ctx, attached.header(), None)
        {
            return ApiRemoteAgentResult::Error(subagent_ownership_error(session_id));
        }

        let shared = {
            let mut resumes = self.resumes.lock();
            if let Some(entry) = resumes.get_mut(session_id) {
                entry.waiters += 1;
                entry.future.clone()
            } else {
                let resume_id = session_id.clone();
                let ctx = self.ctx.clone();
                let options = self.options.clone();
                let future: BoxFuture<'static, Result<Arc<dyn Agent>, ResumeFailure>> =
                    Box::pin(async move {
                        let meta = inspect_cold(&ctx, &resume_id).await?;
                        if has_api_remote_subagent_owner(&ctx, &meta, None) {
                            return Err(ResumeFailure::SubagentOwned(resume_id.clone()));
                        }
                        let ownership_blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let setup = guarded_cold_setup(
                            ctx.clone(),
                            meta,
                            options.setup.clone(),
                            ownership_blocked.clone(),
                        );
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
                            resume_session_id: Some(resume_id.clone()),
                            agent_options: Some((options.agent_options)()),
                            setup: Some(setup),
                        };
                        let handle = registry.resume(options_builder).await.map_err(|error| {
                            if ownership_blocked.load(std::sync::atomic::Ordering::Acquire) {
                                ResumeFailure::SubagentOwned(resume_id.clone())
                            } else {
                                ResumeFailure::Internal(error)
                            }
                        })?;
                        Ok((options.retain_handle)(handle))
                    });
                let shared: SharedResume = future.shared();
                resumes.insert(
                    session_id.clone(),
                    ResumeEntry {
                        future: shared.clone(),
                        waiters: 1,
                    },
                );
                shared
            }
        };

        let _waiter = ResumeWaiter {
            resumes: &self.resumes,
            id: session_id.clone(),
            future: shared.clone(),
        };

        let outcome = shared.clone().await;
        // A shared future coordinates only the in-flight resume. Retaining a
        // successful result would return a disposed Agent after its next idle
        // retirement and silently drop subsequently accepted messages.
        {
            let mut resumes = self.resumes.lock();
            if resumes
                .get(session_id)
                .is_some_and(|current| current.future.ptr_eq(&shared))
            {
                resumes.remove(session_id);
            }
        }
        match outcome {
            Ok(agent) => ApiRemoteAgentResult::Agent(agent),
            Err(failure) => {
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
#[cfg(test)]
mod retirement_gate_tests {
    use super::*;

    #[tokio::test]
    async fn wait_blocks_until_exact_retirement_guard_drops() {
        let gates = Arc::new(RetirementGates::default());
        let id = dsh_session::session_id("retiring");
        let guard = gates.begin(&id);
        let waiter = {
            let gates = Arc::clone(&gates);
            let id = id.clone();
            tokio::spawn(async move { gates.wait(&id).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter released")
            .expect("waiter task");
    }
}

async fn inspect_cold(
    ctx: &Context,
    session_id: &SessionId,
) -> Result<SessionHeader, ResumeFailure> {
    let Some(persistence) = ctx
        .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
        .map(|slot| slot.as_ref().clone())
    else {
        return Err(ResumeFailure::Internal(
            "session persistence is not configured (load a dsh-session-persistence backend)"
                .to_string(),
        ));
    };
    let inspected = persistence
        .read_snapshot(session_id)
        .await
        .map_err(|error| {
            let missing = format!("session \"{session_id}\" not found");
            if error == missing {
                ResumeFailure::SessionNotFound(missing)
            } else {
                ResumeFailure::Internal(format!("cannot restore session \"{session_id}\": {error}"))
            }
        })?
        .ok_or_else(|| {
            ResumeFailure::SessionNotFound(format!("session \"{session_id}\" not found"))
        })?;
    if inspected.header.id != *session_id {
        return Err(ResumeFailure::Internal(format!(
            "session authority does not match requested identity \"{session_id}\""
        )));
    }
    if inspected.header.cwd.is_none() {
        return Err(ResumeFailure::SessionNotFound(format!(
            "session \"{session_id}\" not found"
        )));
    }
    Ok(inspected.header)
}

/// Generation conversion may alter positions and the format number, but
/// cannot change the identity whose header admitted this resume request.
fn same_cold_authority(before: &SessionHeader, prepared: &SessionHeader) -> bool {
    before.id == prepared.id
        && before.created_at == prepared.created_at
        && before.cwd == prepared.cwd
        && before.parent_session == prepared.parent_session
        && before.is_seeded == prepared.is_seeded
        && before.origin == prepared.origin
        && before.delegation_depth.unwrap_or(0) == prepared.delegation_depth.unwrap_or(0)
        && before.agent_preset == prepared.agent_preset
        && (before.version == prepared.version
            || matches!((before.version, prepared.version), (0 | 3, 4)))
}

fn guarded_cold_setup(
    ctx: Context,
    authority: SessionHeader,
    setup: Option<AgentSetup>,
    ownership_blocked: Arc<std::sync::atomic::AtomicBool>,
) -> AgentSetup {
    Arc::new(move |agent_ctx, agent| {
        let ctx = ctx.clone();
        let agent_ctx = agent_ctx.clone();
        let authority = authority.clone();
        let setup = setup.clone();
        let ownership_blocked = ownership_blocked.clone();
        Box::pin(async move {
            let validate = || {
                if has_api_remote_subagent_owner(&ctx, agent.session().header(), Some(&agent)) {
                    ownership_blocked.store(true, std::sync::atomic::Ordering::Release);
                    return Err("the prepared Session is owned by subagent routing".to_string());
                }
                if agent.id() != &authority.id
                    || !same_cold_authority(&authority, agent.session().header())
                {
                    return Err("Session authority changed before Agent publication".to_string());
                }
                Ok(())
            };
            validate()?;
            let commit = match setup {
                Some(setup) => setup(&agent_ctx, agent.clone()).await?,
                None => None,
            };
            // Setup can await external composition. Recheck the exact prepared
            // identity and current ownership before its transaction commits.
            validate()?;
            Ok(commit)
        })
    })
}
