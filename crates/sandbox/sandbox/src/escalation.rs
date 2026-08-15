//! The escalation vocabulary and choreography shared by every
//! sandbox-enforcing tool family (`dsh-tool-bash`, `dsh-tool-fs`): the
//! strictly-wider ladder, the argument-pairing validation, the model-facing
//! denial/hint markers, and [`approve_escalation`] — the ordered fail-closed
//! sequence that resolves a `sandbox_permissions` request through a
//! user-approval channel BEFORE anything executes. Rust port of
//! `packages/sandbox/sandbox/src/escalation.ts`.
//!
//! The channel is a minimal STRUCTURAL function shape
//! ([`EscalationApprover`]), not the approval service type: the tool layer —
//! which owns the agent, the call id, and the tool name — closes over
//! `ctx.approval.request(...)` and hands the closure down, so this package
//! never depends on the approval or agent packages.
//!
//! # Deviations
//!
//! - The TS throws verbatim errors; the Rust port returns
//!   `Result<SandboxMode, String>` with the same verbatim texts.
//! - `AbortSignal` collapses into an optional [`EscalationRequestSignal`]
//!   closure (the Rust cancellation convention used across the port).

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::index::SandboxMode;

/// The strictly-wider table: what a call whose effective mode is the key may
/// escalate TO. Checked at EXECUTION, never baked into a tool schema — the
/// schema's enum is [`ESCALATION_TARGETS`], because schemas are
/// registry-global while the effective mode is per-call truth.
pub static WIDER_MODES: WiderModes = WiderModes;

/// The ladder as one static table (read-only → workspace-write →
/// danger-full-access; `danger-full-access` has no wider mode).
pub struct WiderModes;

impl WiderModes {
    pub fn get(&self, mode: &str) -> Option<&'static [SandboxMode]> {
        match mode {
            "read-only" => Some(&[SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess]),
            "workspace-write" => Some(&[SandboxMode::DangerFullAccess]),
            _ => None,
        }
    }
}

/// The closed escalation-target vocabulary — every mode a call could ever
/// escalate TO (`read-only` is the floor; nothing escalates to it).
/// Advertised whenever the mounted capability confines: cutting the enum
/// down to the modes wider than the composition's DEFAULT would strand a
/// session whose effective mode sits below it.
pub static ESCALATION_TARGETS: &[SandboxMode] = &[SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess];

/// Validate the escalation argument pairing a tool schema cannot express:
/// `sandbox_permissions` and `justification` travel together — an approval
/// prompt without a reason, or a reason driving nothing, is a malformed ask —
/// and the justification must be a non-empty sentence.
pub fn validate_escalation_args(
    sandbox_permissions: Option<&str>,
    justification: Option<&str>,
) -> Result<(), String> {
    if sandbox_permissions.is_some() && justification.is_none() {
        return Err("invalid escalation: sandbox_permissions requires a justification".to_string());
    }
    if justification.is_some() && sandbox_permissions.is_none() {
        return Err(
            "invalid escalation: justification is only valid together with sandbox_permissions"
                .to_string(),
        );
    }
    if justification.is_some_and(|text| text.trim().is_empty()) {
        return Err("invalid justification: expected a non-empty sentence".to_string());
    }
    Ok(())
}

/// The model-facing denial marker — the one vocabulary both enforcing
/// families teach and report, so the model recognizes a policy denial
/// identically whether the kernel refused a bash file effect or the
/// filesystem provider's fence refused a mutation.
pub fn sandbox_denial_marker(mode: SandboxMode) -> String {
    format!("[sandbox: file access denied under {} mode]", mode.as_str())
}

/// The same-turn escalation hint that rides a denial when the composition
/// advertises the escalation fields.
pub fn escalation_hint_marker(subject: &str) -> String {
    format!(
        "[sandbox: escalation available — retry this exact {subject} once with sandbox_permissions \
         (the narrowest wider mode that suffices) + justification; the approval prompt asks the user]"
    )
}

/// The closed outcome vocabulary of one escalation ask — structurally
/// identical to the approval seam's `ApprovalOutcome` (TS
/// `EscalationOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
    Unavailable,
}

/// The minimal approval-request shape [`approve_escalation`] needs —
/// structurally the approval seam's `ApprovalService`, generic over the
/// agent type `A` and call-id type `C` (TS `EscalationApprover`).
pub trait EscalationApprover<A, C>: Send + Sync {
    /// Ask the human to approve one action, resolving to a closed outcome.
    fn request(&self, request: EscalationApproveRequest<A, C>) -> BoxFuture<'static, EscalationOutcome>;
}

/// One approval request handed to the approver (the TS inline request
/// shape).
pub struct EscalationApproveRequest<A, C> {
    pub agent: A,
    pub tool_name: String,
    pub call_id: C,
    pub reason: String,
    pub signal: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// The approval ingredients an escalating tool hands
/// [`approve_escalation`] (TS `EscalationApproval`).
pub struct EscalationApproval<'a, A, C> {
    /// The approval requester (`ctx.approval`), or `None` when none is
    /// composed.
    pub approver: Option<&'a dyn EscalationApprover<A, C>>,
    /// The calling agent, or `None` for an agent-less execution (fails
    /// closed).
    pub agent: Option<A>,
    /// The tool-call id the approval prompt attaches to.
    pub call_id: C,
    /// The tool name recorded on the approval request.
    pub tool_name: String,
    /// The tool-execution abort signal the approval request rides, when
    /// present.
    pub signal: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// One escalation request, as [`approve_escalation`] judges it (TS
/// `EscalationRequest`).
pub struct EscalationRequest {
    /// The requested target mode (schema-pinned to [`ESCALATION_TARGETS`]
    /// when advertised).
    pub requested_mode: String,
    /// The model's one-sentence reason, shown verbatim to the user inside
    /// the audit reason.
    pub justification: String,
    /// The call's effective mode (session override ?? composition default)
    /// the request must strictly widen.
    pub effective_mode: SandboxMode,
    /// The family's noun for the escalated action in user-facing texts
    /// (`command` for bash, `operation` for fs).
    pub subject: String,
}

/// Resolve a sandbox-escalation request BEFORE anything executes: check
/// strict widening against the call's effective mode, then resolve the
/// approval channel, then map every outcome — the ordered fail-closed
/// sequence both enforcing families share. Returns the granted mode to stamp
/// onto exactly this call; returns the distinct verbatim text for every
/// other path (a non-widening request, a missing approval service, an
/// agent-less execution, a rejection, a cancellation, an unanswerable ask) —
/// the tool registry turns the error into the call's isError result, and
/// nothing has run. A non-widening request never prompts a human.
pub async fn approve_escalation<A: Send + Sync + 'static, C: Send + Sync + 'static>(
    request: EscalationRequest,
    approval: EscalationApproval<'_, A, C>,
) -> Result<SandboxMode, String> {
    let EscalationRequest { requested_mode, justification, effective_mode, subject } = request;
    // Strict widening is an EXECUTION check against the call's effective
    // mode — deliberately not a schema constraint.
    let wider = WIDER_MODES.get(effective_mode.as_str()).unwrap_or(&[]);
    let mode: SandboxMode = match requested_mode.as_str() {
        "read-only" => SandboxMode::ReadOnly,
        "workspace-write" => SandboxMode::WorkspaceWrite,
        "danger-full-access" => SandboxMode::DangerFullAccess,
        // Unknown spellings fail the ladder check below with the verbatim
        // non-widening text (the TS `WIDER_MODES[mode] ?? []` path).
        _ => SandboxMode::ReadOnly,
    };
    if !wider.contains(&mode) {
        return Err(format!(
            "sandbox escalation to \"{requested_mode}\" is not strictly wider than this call's current \"{}\" mode",
            effective_mode.as_str()
        ));
    }
    let Some(approver) = approval.approver else {
        return Err(format!(
            "sandbox escalation to \"{requested_mode}\" requires approval, but no approval service is composed"
        ));
    };
    let Some(agent) = approval.agent else {
        return Err(format!(
            "sandbox escalation to \"{requested_mode}\" requires approval, but the call has no agent to route it through"
        ));
    };
    // Self-contained for the audit trail: approval/asked stores this reason,
    // and the target mode is part of the grant's identity.
    let outcome = approver
        .request(EscalationApproveRequest {
            agent,
            tool_name: approval.tool_name,
            call_id: approval.call_id,
            reason: format!("escalate sandbox to {requested_mode}: {justification}"),
            signal: approval.signal,
        })
        .await;
    match outcome {
        // The schema enum already pinned `mode` to the closed target
        // vocabulary; the check above proved it is strictly wider.
        EscalationOutcome::AllowedOnce => Ok(mode),
        EscalationOutcome::Rejected => Err(format!(
            "the user rejected escalating this {subject} to \"{requested_mode}\""
        )),
        EscalationOutcome::Cancelled => Err(format!(
            "approval for escalating to \"{requested_mode}\" was cancelled"
        )),
        EscalationOutcome::Unavailable => Err(format!(
            "sandbox escalation to \"{requested_mode}\" requires approval, but no approval channel is available"
        )),
    }
}
