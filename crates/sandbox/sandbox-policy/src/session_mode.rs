//! Per-session sandbox-mode override: the session log as the store. Rust
//! port of `packages/sandbox/sandbox-policy/src/session-mode.ts`. A runtime
//! switch is recorded as one `sandbox/mode` event on the session it applies
//! to; `effective = fold(events) ?? the deployment default`, so an override
//! survives restart by replay, two sessions can never see each other's
//! state, and there is no external config store. The event is log-only.

use dsh_sandbox::SandboxMode;
use dsh_session::{Session, SessionEvent};
use serde_json::json;

/// Every [`SandboxMode`], for option advertisement and runtime validation of
/// untrusted mode strings.
pub static SANDBOX_MODES: &[SandboxMode] = &[
    SandboxMode::ReadOnly,
    SandboxMode::WorkspaceWrite,
    SandboxMode::DangerFullAccess,
];

/// The session's sandbox-mode override: the last `sandbox/mode` event in the
/// log, or `None` when the session never switched (callers apply the
/// deployment default). The pure fold — resume needs no catch-up machinery
/// because replaying the log IS the state.
pub fn effective_sandbox_mode(events: &[SessionEvent]) -> Option<SandboxMode> {
    for event in events.iter().rev() {
        if event.type_ == "sandbox/mode" {
            return match event.data.get("mode").and_then(|mode| mode.as_str()) {
                Some("read-only") => Some(SandboxMode::ReadOnly),
                Some("workspace-write") => Some(SandboxMode::WorkspaceWrite),
                Some("danger-full-access") => Some(SandboxMode::DangerFullAccess),
                _ => None,
            };
        }
    }
    None
}

/// THE write path for a session's sandbox-mode override: appends exactly one
/// `sandbox/mode` event — the switch IS its event; nothing mutates mode
/// state out of band.
pub fn set_sandbox_mode(session: &Session, mode: SandboxMode) -> Result<SessionEvent, String> {
    session.append("sandbox/mode", json!({ "mode": mode.as_str() }), None)
}
