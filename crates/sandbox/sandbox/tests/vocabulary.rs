//! Rust port of the TS `vocabulary.spec.ts` + `escalation.spec.ts` suites
//! for the sandbox seam: the fail-closed error's structured identity, the
//! strictly-wider ladder, argument-pairing validation, the model-facing
//! markers, and `approveEscalation`'s ordered fail-closed sequence.
//!
//! Deviations:
//!
//! - The TS throws; the Rust port returns `Err(String)` with the same
//!   verbatim texts.
//! - The "outcome outside the closed union" defensiveness is inexpressible
//!   (the Rust outcome is a closed enum; the TS `assertNever` guard has no
//!   counterpart).

use std::sync::Arc;

use dsh_sandbox::{
    ESCALATION_TARGETS, EscalationApproval, EscalationApproveRequest, EscalationApprover,
    EscalationOutcome, SandboxMode, SandboxUnavailableError, WIDER_MODES, approve_escalation,
    escalation_hint_marker, sandbox_denial_marker, validate_escalation_args,
};
use dsh_sandbox::{SANDBOX_UNAVAILABLE, ConfinedSandboxMode};
use futures::future::BoxFuture;

// ---------------------------------------------------------------------------
// vocabulary

#[test]
fn sandbox_unavailable_error_carries_the_structured_identity() {
    let error = SandboxUnavailableError::new(ConfinedSandboxMode::ReadOnly, None);
    assert_eq!(error.code(), SANDBOX_UNAVAILABLE);
    assert_eq!(format!("{error:?}").contains("SandboxUnavailableError"), true);
}

#[test]
fn sandbox_unavailable_error_names_the_refused_mode_and_the_escape_hatches() {
    let error = SandboxUnavailableError::new(ConfinedSandboxMode::WorkspaceWrite, None);
    let message = error.to_string();
    assert!(message.contains("\"workspace-write\""), "{message}");
    assert!(message.contains("danger-full-access"), "{message}");
    assert!(!message.contains("Runner failure"), "{message}");
}

#[test]
fn sandbox_unavailable_error_carries_the_runner_detail_at_execution_time() {
    let error = SandboxUnavailableError::new(
        ConfinedSandboxMode::ReadOnly,
        Some("landlock-run: landlock is not enforced by this kernel"),
    );
    assert_eq!(error.code(), SANDBOX_UNAVAILABLE);
    assert!(
        error
            .to_string()
            .contains("Runner failure: landlock-run: landlock is not enforced by this kernel"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// the strictly-wider ladder

#[test]
fn the_ladder_read_only_escalates_to_either_wider_mode() {
    assert_eq!(
        WIDER_MODES.get("read-only"),
        Some(&[SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess][..])
    );
    assert_eq!(
        WIDER_MODES.get("workspace-write"),
        Some(&[SandboxMode::DangerFullAccess][..])
    );
    assert_eq!(WIDER_MODES.get("danger-full-access"), None);
}

#[test]
fn the_target_enum_is_the_closed_set_every_session_could_escalate_to() {
    assert_eq!(ESCALATION_TARGETS, &[SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess]);
}

// ---------------------------------------------------------------------------
// validateEscalationArgs

#[test]
fn validate_escalation_args_accepts_neither_or_both_with_a_reason() {
    assert!(validate_escalation_args(None, None).is_ok());
    assert!(
        validate_escalation_args(Some("workspace-write"), Some("because the workspace needs it"))
            .is_ok()
    );
}

#[test]
fn validate_escalation_args_rejects_mismatched_and_blank_shapes() {
    let error = validate_escalation_args(Some("workspace-write"), None)
        .err()
        .expect("rejects");
    assert!(error.contains("requires a justification"), "{error}");
    let error = validate_escalation_args(None, Some("orphan reason")).err().expect("rejects");
    assert!(error.contains("only valid together with sandbox_permissions"), "{error}");
    let error = validate_escalation_args(Some("workspace-write"), Some("   "))
        .err()
        .expect("rejects");
    assert!(error.contains("non-empty sentence"), "{error}");
}

// ---------------------------------------------------------------------------
// the model-facing markers

#[test]
fn the_denial_marker_names_the_mode() {
    assert_eq!(
        sandbox_denial_marker(SandboxMode::ReadOnly),
        "[sandbox: file access denied under read-only mode]"
    );
    assert_eq!(
        sandbox_denial_marker(SandboxMode::WorkspaceWrite),
        "[sandbox: file access denied under workspace-write mode]"
    );
}

#[test]
fn the_hint_marker_names_the_family_subject() {
    assert!(escalation_hint_marker("command")
        .contains("retry this exact command once with sandbox_permissions"));
    assert!(escalation_hint_marker("operation")
        .contains("retry this exact operation once with sandbox_permissions"));
}

// ---------------------------------------------------------------------------
// approveEscalation

type Agent = serde_json::Value;
type CallId = String;

/// An approver that records the request and returns a fixed outcome.
struct FixedApprover {
    outcome: EscalationOutcome,
    seen: Arc<parking_lot::Mutex<Vec<EscalationApproveRequest<Agent, CallId>>>>,
}

impl EscalationApprover<Agent, CallId> for FixedApprover {
    fn request(
        &self,
        request: EscalationApproveRequest<Agent, CallId>,
    ) -> BoxFuture<'static, EscalationOutcome> {
        let outcome = self.outcome;
        let seen = self.seen.clone();
        Box::pin(async move {
            seen.lock().push(request);
            outcome
        })
    }
}

fn request() -> dsh_sandbox::EscalationRequest {
    dsh_sandbox::EscalationRequest {
        requested_mode: "workspace-write".to_string(),
        justification: "the user asked to write in the workspace".to_string(),
        effective_mode: SandboxMode::ReadOnly,
        subject: "command".to_string(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn grants_returning_the_requested_mode_through_the_approver_with_the_audit_reason() {
    let seen: Arc<parking_lot::Mutex<Vec<EscalationApproveRequest<Agent, CallId>>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let approver = FixedApprover { outcome: EscalationOutcome::AllowedOnce, seen: seen.clone() };
    let approval = EscalationApproval {
        approver: Some(&approver),
        agent: Some(serde_json::json!({})),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        signal: None,
    };
    let granted = approve_escalation(request(), approval).await.expect("grants");
    assert_eq!(granted, SandboxMode::WorkspaceWrite);
    let seen = seen.lock();
    assert_eq!(
        seen[0].reason,
        "escalate sandbox to workspace-write: the user asked to write in the workspace"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_widening_request_fails_closed_with_its_own_text_and_never_asks() {
    let seen: Arc<parking_lot::Mutex<Vec<EscalationApproveRequest<Agent, CallId>>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let approver = FixedApprover { outcome: EscalationOutcome::AllowedOnce, seen: seen.clone() };
    let mut narrower = request();
    narrower.requested_mode = "read-only".to_string();
    let approval = EscalationApproval {
        approver: Some(&approver),
        agent: Some(serde_json::json!({})),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        signal: None,
    };
    let error = approve_escalation(narrower, approval).await.err().expect("rejects");
    assert!(
        error.contains("not strictly wider than this call's current \"read-only\" mode"),
        "{error}"
    );
    assert!(seen.lock().is_empty());

    let mut full = request();
    full.effective_mode = SandboxMode::DangerFullAccess;
    let approval = EscalationApproval {
        approver: Some(&approver),
        agent: Some(serde_json::json!({})),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        signal: None,
    };
    let error = approve_escalation(full, approval).await.err().expect("rejects");
    assert!(error.contains("not strictly wider"), "{error}");
    assert!(seen.lock().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_approval_service_and_an_agent_less_call_each_fail_closed() {
    let missing = EscalationApproval {
        approver: None,
        agent: Some(serde_json::json!({})),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        signal: None,
    };
    let error = approve_escalation(request(), missing).await.err().expect("rejects");
    assert!(error.contains("no approval service is composed"), "{error}");

    let approver = FixedApprover {
        outcome: EscalationOutcome::AllowedOnce,
        seen: Arc::new(parking_lot::Mutex::new(Vec::new())),
    };
    let agentless = EscalationApproval {
        approver: Some(&approver),
        agent: None,
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        signal: None,
    };
    let error = approve_escalation(request(), agentless).await.err().expect("rejects");
    assert!(error.contains("no agent to route it through"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn maps_each_non_grant_outcome_to_its_distinct_verbatim_text() {
    let cases = [
        (
            EscalationOutcome::Rejected,
            "operation",
            "the user rejected escalating this operation to \"workspace-write\"",
        ),
        (
            EscalationOutcome::Cancelled,
            "command",
            "approval for escalating to \"workspace-write\" was cancelled",
        ),
        (
            EscalationOutcome::Unavailable,
            "command",
            "no approval channel is available",
        ),
    ];
    for (outcome, subject, needle) in cases {
        let approver = FixedApprover {
            outcome,
            seen: Arc::new(parking_lot::Mutex::new(Vec::new())),
        };
        let mut request = request();
        request.subject = subject.to_string();
        let approval = EscalationApproval {
            approver: Some(&approver),
            agent: Some(serde_json::json!({})),
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            signal: None,
        };
        let error = approve_escalation(request, approval).await.err().expect("rejects");
        assert!(error.contains(needle), "{error}");
    }
}
