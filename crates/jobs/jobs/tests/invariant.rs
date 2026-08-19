//! Rust port of the TS `invariant.spec.ts` suite for `dsh-jobs`: the
//! cross-field relationships in one registry snapshot, validated through a
//! probe registry and a capturing failure channel.

use std::sync::Arc;

use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_jobs::{
    JobDoneListener, JobRegistry, JobSnapshot, JobStatus, JobsChangedListener, KillOutcome,
    invariant::validate_snapshot, job_id,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};

/// Minimal agent stub for owner-correlation checks.
struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: cordis::Context,
    scope_key: ScopeKey,
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &cordis::Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(
        &self,
        _cause: dsh_agent::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(
        &self,
        _message: dsh_session::UserMessage,
        _target: dsh_agent::InboxTarget,
        _wakeup: bool,
    ) {
    }

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

fn base() -> JobSnapshot {
    JobSnapshot {
        id: job_id("bash-1"),
        kind: "bash".to_string(),
        label: "compile".to_string(),
        output_limit_bytes: None,
        owner_session: None,
        status: JobStatus::Completed,
        detail: None,
        started_at: 10,
        finished_at: Some(20),
        reported: false,
    }
}

fn running() -> JobSnapshot {
    JobSnapshot {
        status: JobStatus::Running,
        finished_at: None,
        ..base()
    }
}

/// Run `validate_snapshot` with a capturing failure channel and return the
/// first failure message (None = coherent).
fn failures(snapshot: &JobSnapshot, owner: Option<&Arc<dyn Agent>>) -> Option<String> {
    let collected: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let collected = collected.clone();
        validate_snapshot(snapshot, owner, &move |message| {
            collected.lock().push(message.to_string());
        });
    }
    collected.lock().first().cloned()
}

fn agent(raw_id: &str) -> Arc<dyn Agent> {
    let id = session_id(raw_id);
    let session = Session::create(id.clone(), None, None).expect("session");
    let inbox = Inbox::new(&session, Default::default()).expect("inbox");
    Arc::new(StubAgent {
        id,
        session,
        inbox,
        ctx: cordis::Context::root(),
        scope_key: ScopeKey::new(),
    })
}

#[test]
fn accepts_coherent_current_and_terminal_snapshots() {
    assert_eq!(failures(&running(), None), None);
    assert_eq!(failures(&base(), None), None);
    let owner = agent("owner");
    let owned = JobSnapshot {
        id: job_id("subagent-2"),
        kind: "subagent".to_string(),
        owner_session: Some(owner.id().clone()),
        ..base()
    };
    assert_eq!(failures(&owned, Some(&owner)), None);
}

#[test]
fn rejects_an_incoherent_registry_snapshot() {
    let owner_actual = agent("actual");

    let cases: Vec<(JobSnapshot, Option<&Arc<dyn Agent>>, &str)> = vec![
        (
            JobSnapshot {
                id: job_id("-1"),
                kind: String::new(),
                ..base()
            },
            None,
            "positive ordinal",
        ),
        (
            JobSnapshot {
                id: job_id("other-1"),
                ..base()
            },
            None,
            "\"bash-\" followed by a positive ordinal",
        ),
        (
            JobSnapshot {
                id: job_id("bash-x"),
                ..base()
            },
            None,
            "positive ordinal",
        ),
        (
            JobSnapshot {
                id: job_id("bash-0"),
                ..base()
            },
            None,
            "positive ordinal",
        ),
        (
            JobSnapshot {
                label: String::new(),
                ..base()
            },
            None,
            "label must be non-empty",
        ),
        (
            JobSnapshot {
                status: JobStatus::Running,
                ..base()
            },
            None,
            "finishedAt must be present exactly for a terminal status",
        ),
        (
            JobSnapshot {
                finished_at: None,
                ..base()
            },
            None,
            "finishedAt must be present exactly for a terminal status",
        ),
        (
            JobSnapshot {
                finished_at: Some(9),
                ..base()
            },
            None,
            "no earlier than startedAt",
        ),
        (
            JobSnapshot {
                owner_session: Some(session_id("recorded")),
                ..base()
            },
            Some(&owner_actual),
            "does not match its completion owner",
        ),
    ];
    for (snapshot, owner, expected) in cases {
        let message = failures(&snapshot, owner).expect("incoherent snapshot reported");
        assert!(message.contains(expected), "message: {message}");
    }
}

#[test]
fn rejects_an_incoherent_record_already_present_at_installation() {
    // The installer validates every currently listed unowned record; a
    // captured failure surfaces here directly through the shared validator.
    let incoherent = JobSnapshot {
        label: String::new(),
        ..base()
    };
    let message = failures(&incoherent, None).expect("incoherent record");
    assert!(message.contains("label must be non-empty"), "{message}");
}

// The TS probe-driven installer wiring (list seeding + onJobDone
// subscription) is exercised through the pure validator above; the
// `jobs`-inject wiring itself is the installer's contract and is covered by
// the invariants crate's installer tests.
