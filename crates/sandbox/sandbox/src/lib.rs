//! Same-world process-confinement capability seam (`ctx.sandbox`). Rust port
//! of `@deepseek-ai/dsh-sandbox`.

pub mod escalation;
pub mod index;
pub mod invariant;
pub mod roots;

pub use escalation::{
    ESCALATION_TARGETS, EscalationApproval, EscalationApproveRequest, EscalationApprover,
    EscalationOutcome, EscalationRequest, WIDER_MODES, approve_escalation, escalation_hint_marker,
    sandbox_denial_marker, validate_escalation_args,
};
pub use index::{
    ConfinedArgv, ConfinedSandboxMode, RunnerFailureRule, SANDBOX_UNAVAILABLE, SandboxEnforcement,
    SandboxExecutionPolicy, SandboxMode, SandboxPolicy, SandboxProvider, SandboxProviderRef,
    SandboxUnavailableError,
};
pub use roots::{canonical_path, writable_roots};
