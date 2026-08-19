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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::session_id;

    fn event(type_: &str, mode: &str) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq: 0,
            time: 0,
            data: json!({ "mode": mode }),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[test]
    fn the_mode_vocabulary_lists_every_mode() {
        assert_eq!(
            SANDBOX_MODES,
            &[
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::DangerFullAccess
            ]
        );
    }

    #[test]
    fn the_fold_returns_the_last_switch_or_none_without_one() {
        assert_eq!(effective_sandbox_mode(&[]), None);
        assert_eq!(
            effective_sandbox_mode(&[
                event("sandbox/mode", "read-only"),
                event("user/message", "ignored"),
                event("sandbox/mode", "workspace-write"),
            ]),
            Some(SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            effective_sandbox_mode(&[
                event("sandbox/mode", "workspace-write"),
                event("sandbox/mode", "danger-full-access"),
            ]),
            Some(SandboxMode::DangerFullAccess)
        );
    }

    #[test]
    fn set_sandbox_mode_appends_exactly_one_event_per_switch() {
        let session = Session::create(session_id("s1"), None, None).expect("session");
        let before = session.events().len();
        set_sandbox_mode(&session, SandboxMode::ReadOnly).expect("append");
        set_sandbox_mode(&session, SandboxMode::WorkspaceWrite).expect("append");
        let events = session.events();
        assert_eq!(events.len(), before + 2);
        assert_eq!(events[before].type_, "sandbox/mode");
        assert_eq!(
            effective_sandbox_mode(&events),
            Some(SandboxMode::WorkspaceWrite)
        );
    }
}
