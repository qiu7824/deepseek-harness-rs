//! Service Definition for the approval capability seam, covering requests,
//! cancellation, audit, and per-session policy. Missing answerers fail
//! closed; grants apply only to the requested action.
//! Rust port of `packages/interaction/user-approval/src/index.ts`
//! (+ `types.ts`).
//!
//! # Deviations
//!
//! - The abort seam is a predicate: an aborted ask settles `cancelled`.
//! - The runtime policy-context contribution resolves to the empty string
//!   (the TS no-agent branch) because the Rust [`dsh_system_prompt::AssembleContext`]
//!   does not carry the assembling agent yet.
//! - Answerer failures are contained per listener: a throwing answerer
//!   settles the question `unavailable` (fail closed), exactly like the TS
//!   seam.
//! - `effectiveApprovalPolicy` normalizes an unknown logged policy to
//!   `None` (the TS fold returns the raw string; the invariant companion
//!   flags unknown policies anyway).

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, BoxFuture, Context, InjectSpec, Plugin, PluginError, arc, downcast_arc};
use dsh_agent::Agent;
use dsh_brand::Branded;
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use dsh_scope::scope_target;
use dsh_session::{Session, SessionEvent};
use dsh_system_prompt::{PromptContext, PromptText, SystemPrompt};

/// The brand marker for [`ApprovalRequestId`].
#[doc(hidden)]
pub enum ApprovalRequestIdTag {}

/// Pairs one `approval/asked` audit event with its `approval/decided`.
pub type ApprovalRequestId = Branded<ApprovalRequestIdTag>;

/// Brand a string as an [`ApprovalRequestId`].
pub fn approval_request_id(id: impl Into<String>) -> ApprovalRequestId {
    ApprovalRequestId::new(id)
}

/// Closed approval outcomes: a one-shot grant, explicit rejection, withdrawn
/// request, or unavailable answerer. Callers fail closed on `unavailable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
    Unavailable,
}

impl ApprovalOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalOutcome::AllowedOnce => "allowed-once",
            ApprovalOutcome::Rejected => "rejected",
            ApprovalOutcome::Cancelled => "cancelled",
            ApprovalOutcome::Unavailable => "unavailable",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "allowed-once" => Some(ApprovalOutcome::AllowedOnce),
            "rejected" => Some(ApprovalOutcome::Rejected),
            "cancelled" => Some(ApprovalOutcome::Cancelled),
            "unavailable" => Some(ApprovalOutcome::Unavailable),
            _ => None,
        }
    }
}

/// A session's approval policy.
///
/// - [`ApprovalPolicy::Ask`] delegates to the composed answerers; without an
///   answerer the chain falls through to the fail-closed `unavailable`.
/// - [`ApprovalPolicy::Never`] never prompts anyone: every ask resolves
///   `rejected`. Decided inside the service's own request path (not a
///   listener gate), so registration order can never bypass the policy whose
///   outcome is knowable without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Ask,
    Never,
}

impl ApprovalPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalPolicy::Ask => "ask",
            ApprovalPolicy::Never => "never",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(ApprovalPolicy::Ask),
            "never" => Some(ApprovalPolicy::Never),
            _ => None,
        }
    }
}

/// Every [`ApprovalPolicy`], for option advertisement and runtime validation
/// of untrusted policy strings (TS `APPROVAL_POLICIES`).
pub const APPROVAL_POLICIES: &[ApprovalPolicy] = &[ApprovalPolicy::Ask, ApprovalPolicy::Never];

/// The cancellation seam (TS `AbortSignal`).
pub type ApprovalAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Model-facing statement for the deterministic `never` policy.
pub const NEVER_SENTENCE: &str = "Approval prompts are disabled in this session: actions that require approval are rejected automatically — do not request sandbox escalation (do not set `sandbox_permissions`).";
/// Model-facing statement for an interactive policy that may still fail
/// closed.
pub const ASK_SENTENCE: &str = "Approval policy: ask. Operations that require approval may ask through the configured answerers; without an available answerer, the request fails closed.";

/// The session's approval-policy override: the last `approval/policy` event
/// in the log (TS `effectiveApprovalPolicy`).
pub fn effective_approval_policy(events: &[SessionEvent]) -> Option<ApprovalPolicy> {
    for event in events.iter().rev() {
        if event.type_ == "approval/policy" {
            return event
                .data
                .get("policy")
                .and_then(|value| value.as_str())
                .and_then(ApprovalPolicy::from_str);
        }
    }
    None
}

/// Whether the log currently sits inside an open turn (TS `hasOpenTurn`).
pub fn has_open_turn(events: &[SessionEvent]) -> bool {
    for event in events.iter().rev() {
        match event.type_.as_str() {
            "turn/start" => return true,
            "turn/end" => return false,
            _ => {}
        }
    }
    false
}

/// Append the sole durable representation of a session policy override (TS
/// `setApprovalPolicy`; the Rust append is fallible, so the result carries
/// the committed [`SessionEvent`]).
pub fn set_approval_policy(
    session: &Session,
    policy: ApprovalPolicy,
) -> Result<SessionEvent, String> {
    session.append(
        "approval/policy",
        serde_json::json!({ "policy": policy.as_str() }),
        None,
    )
}

/// Readonly same-process permission question.
#[derive(Clone)]
pub struct ApprovalRequest {
    /// The agent on whose behalf the question is asked.
    pub agent: Arc<dyn Agent>,
    /// The tool the question is about (presentation and audit).
    pub tool_name: String,
    /// The exact tool call being decided, when the asker has one.
    pub call_id: Option<String>,
    /// The asker's human-readable explanation of WHY it is asking.
    pub reason: Option<String>,
    /// Aborting withdraws the question.
    pub signal: Option<ApprovalAbort>,
}

/// Plugin config (TS `ApprovalService.Config`; the schema defaults an omitted
/// policy to `ask`, which [`ApprovalService::effective_policy`] applies).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The deployment's default [`ApprovalPolicy`].
    pub policy: Option<ApprovalPolicy>,
}

/// Approval service that applies session policy before answerers and logs
/// every ask/outcome pair to the requesting session (TS
/// `ApprovalService`).
pub struct ApprovalService {
    ctx: Context,
    config: Config,
}

impl ApprovalService {
    /// Create the service, register it as `approval`, and mount the
    /// system-prompt policy context (TS constructor + `ctx.inject`).
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            config,
        });
        ctx.register_service(service.clone());

        // TS: ctx.inject(['systemPrompt'], scope =>
        //   scope.systemPrompt.context({ name: 'approval:policy', order: 115,
        //     text: ctx => ctx.agent === undefined ? '' : sentence(ctx) }))
        // The Rust AssembleContext carries no agent (dsh-agent deviation), so
        // the contribution resolves to the TS no-agent empty branch.
        let inject = Arc::new(
            move |scope: &Context,
                  _config: ArcValue|
                  -> BoxFuture<'static, Result<(), PluginError>> {
                let scope = scope.clone();
                Box::pin(async move {
                    let Some(system_prompt) = scope
                        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
                        .map(|slot| slot.as_ref().clone())
                    else {
                        return Ok(());
                    };
                    let disposer = system_prompt.context(
                        &scope,
                        PromptContext {
                            name: "approval:policy".to_string(),
                            order: 115.0,
                            text: PromptText::Static(String::new()),
                        },
                    );
                    // Attach the removal to the inject fiber so plugin stop
                    // disposes the contribution (TS ctx.effect semantics).
                    let _ = scope.effect(
                        "approval:policy context",
                        Box::pin(async move { Some(disposer) }),
                    );
                    Ok(())
                })
            },
        );
        let _fiber = ctx.inject(InjectSpec::new(["systemPrompt"]), inject);
        service
    }

    /// The deployment config this service resolves under (TS `public
    /// config`; the permission-presets seam reads `config.policy`).
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Switch one live agent's policy and queue the transition for its next
    /// model step (TS `setPolicy`).
    pub fn set_policy(&self, agent: &Arc<dyn Agent>, policy: ApprovalPolicy) -> Result<(), String> {
        let previous = self.effective_policy(agent.session());
        if previous == policy {
            return Ok(());
        }
        set_approval_policy(agent.session(), policy)?;
        agent.inject(create_user_message(
            vec![ContentBlock::Text {
                text: format!(
                    "The approval policy changed from \"{}\" to \"{}\" (changed by the user).",
                    previous.as_str(),
                    policy.as_str()
                ),
            }],
            MessageSource::Plugin {
                plugin: "user-approval".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ));
        Ok(())
    }

    /// The session's effective policy: its own `approval/policy` fold, else
    /// the configured default, else `ask` (TS `effectivePolicy`).
    pub fn effective_policy(&self, session: &Session) -> ApprovalPolicy {
        self.override_of(session)
            .or(self.config.policy)
            .unwrap_or(ApprovalPolicy::Ask)
    }

    /// Read the session override without applying the configured default (TS
    /// `overrideOf`).
    pub fn override_of(&self, session: &Session) -> Option<ApprovalPolicy> {
        effective_approval_policy(&session.events())
    }

    /// Ask the composed answerers to decide one readonly same-process
    /// request (TS `request`).
    pub async fn request(&self, req: &ApprovalRequest) -> Result<ApprovalOutcome, String> {
        let session = req.agent.session();
        if !has_open_turn(&session.events()) {
            return Err(
                "approval.request() outside an open turn: the approval/asked + approval/decided audit pair must be turn-enclosed (a bare event between turns is crash-tail garbage on reload). Ask from inside the turn that needs the decision."
                    .to_string(),
            );
        }
        let id = approval_request_id(uuid::Uuid::new_v4().to_string());
        let mut asked = serde_json::json!({
            "id": id.as_str(),
            "toolName": req.tool_name,
        });
        if let Some(call_id) = &req.call_id {
            asked["callId"] = serde_json::json!(call_id);
        }
        if let Some(reason) = &req.reason {
            asked["reason"] = serde_json::json!(reason);
        }
        session.append("approval/asked", asked, None)?;
        let outcome = self.decide(req, session).await;
        session.append(
            "approval/decided",
            serde_json::json!({ "id": id.as_str(), "outcome": outcome.as_str() }),
            None,
        )?;
        Ok(outcome)
    }

    /// Dispatch the scoped waterfall, contained and raced against the request
    /// signal (TS `decide`).
    async fn decide(&self, req: &ApprovalRequest, session: &Session) -> ApprovalOutcome {
        if req.signal.as_ref().is_some_and(|signal| signal()) {
            return ApprovalOutcome::Cancelled;
        }
        // The 'never' policy is decided HERE, before any dispatch: a listener
        // registered with prepend after this service mounts would sit ahead
        // of any gate LISTENER, so only the service's own request path can
        // keep the documented promise that 'never' rejects deterministically
        // regardless of registration order.
        if self.effective_policy(session) == ApprovalPolicy::Never {
            return ApprovalOutcome::Rejected;
        }
        let carrier = scope_target(None, Some(req.agent.scope_key().clone()));
        let dispatch_ctx = self.ctx.with_filter(carrier.filter);
        let fallback: BoxFuture<'static, ArcValue> =
            Box::pin(async { arc(ApprovalOutcome::Unavailable) });
        let payload = arc(req.clone());
        let answer = async move {
            let dispatch = dispatch_ctx.waterfall("approval/request", vec![payload], fallback);
            // Contain a throwing answerer (sync or async): the question fails
            // closed, never the caller's tool call.
            match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(dispatch)).await {
                // Normalize a rogue (non-vocabulary) answerer return to the
                // fail-closed outcome instead of leaking it into callers'
                // closed-union switches.
                Ok(value) => downcast_arc::<ApprovalOutcome>(&value)
                    .map(|outcome| outcome.as_ref().clone())
                    .unwrap_or(ApprovalOutcome::Unavailable),
                Err(_) => ApprovalOutcome::Unavailable,
            }
        };
        let Some(signal) = &req.signal else {
            return answer.await;
        };
        let poller = {
            let signal = signal.clone();
            async move {
                loop {
                    if signal() {
                        return ApprovalOutcome::Cancelled;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                }
            }
        };
        tokio::pin!(answer);
        tokio::pin!(poller);
        tokio::select! {
            biased;
            outcome = &mut answer => outcome,
            // After an abort wins the race the late answer is discarded by
            // construction: the answer future is dropped.
            _ = &mut poller => ApprovalOutcome::Cancelled,
        }
    }
}

impl cordis::Service for ApprovalService {
    fn service_name(&self) -> &'static str {
        "approval"
    }
}

/// The Cordis plugin form (TS mounts the service class with the schema).
pub struct ApprovalPlugin {
    config: Config,
}

impl ApprovalPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for ApprovalPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("user-approval")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        ApprovalService::install(ctx, self.config.clone());
        Ok(())
    }
}
