use crate::*;
use cordis::{BoxFuture, Context};
use dsh_agent::{
    Agent, AgentCancelCause, AgentControlBusy, AgentControlGuard, AgentOptions, AgentRegistry,
    AgentStatus, CancelOptions, Inbox, InboxTarget,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, SurfaceIntent, SurfaceOp, UserMessage};
use parking_lot::{Mutex, ReentrantMutex, ReentrantMutexGuard};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

struct TestAgent {
    ctx: Context,
    session: Session,
    inbox: Inbox,
    key: ScopeKey,
    options: AgentOptions,
    generation: ReentrantMutex<Cell<u64>>,
    busy: Arc<AtomicBool>,
}
struct Control<'a> {
    _guard: ReentrantMutexGuard<'a, Cell<u64>>,
}
impl AgentControlGuard for Control<'_> {}
impl Agent for TestAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
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
        if self.busy.load(Ordering::SeqCst) {
            AgentStatus::Running
        } else {
            AgentStatus::Idle
        }
    }
    fn try_idle_control(&self) -> Result<Box<dyn AgentControlGuard + '_>, AgentControlBusy> {
        let guard = self
            .generation
            .try_lock()
            .ok_or(AgentControlBusy::Contended)?;
        if self.busy.load(Ordering::SeqCst) {
            return Err(AgentControlBusy::Active);
        }
        if self.inbox.has_pending() {
            return Err(AgentControlBusy::PendingInput);
        }
        Ok(Box::new(Control { _guard: guard }))
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &ScopeKey {
        &self.key
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {
        let guard = self.generation.lock();
        guard.set(guard.get() + 1);
    }
    fn cancellation_generation(&self) -> Option<u64> {
        Some(self.generation.lock().get())
    }
    fn try_generation_control(
        &self,
        expected: u64,
    ) -> Result<Box<dyn AgentControlGuard + '_>, AgentControlBusy> {
        let guard = self
            .generation
            .try_lock()
            .ok_or(AgentControlBusy::Contended)?;
        if guard.get() != expected {
            return Err(AgentControlBusy::Contended);
        }
        Ok(Box::new(Control { _guard: guard }))
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        task()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

struct Fixture {
    ctx: Context,
    goals: Arc<GoalService>,
    agent: Arc<dyn Agent>,
    detach: cordis::Disposer,
    busy: Arc<AtomicBool>,
}
impl Fixture {
    async fn new() -> Self {
        let ctx = Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let registry = AgentRegistry::install(&ctx);
        let goals = GoalService::install(&ctx, Config::default());
        let session = store
            .create(
                &ctx,
                Some(dsh_session::session_id("goal-requirements")),
                None,
            )
            .await
            .unwrap();
        let busy = Arc::new(AtomicBool::new(false));
        let agent: Arc<dyn Agent> = Arc::new(TestAgent {
            ctx: ctx.clone(),
            inbox: Inbox::new(&session, Default::default()).unwrap(),
            session,
            key: ScopeKey::new(),
            options: AgentOptions::default(),
            generation: ReentrantMutex::new(Cell::new(0)),
            busy: busy.clone(),
        });
        let detach = registry.enter(agent.clone(), None).unwrap();
        Self {
            ctx,
            goals,
            agent,
            detach,
            busy,
        }
    }
    fn create(&self, text: &str) -> GoalView {
        self.goals
            .create(
                &self.agent,
                CreateGoalRequest {
                    objective: text.into(),
                    max_goal_rounds: Some(8),
                },
            )
            .unwrap()
    }
    fn identity(&self) -> GoalRequirementsIdentity {
        self.goals
            .requirements_identity(&self.agent)
            .unwrap()
            .unwrap()
    }
    fn install_guard(&self, guard: Arc<dyn GoalCompletionGuard>) {
        self.ctx.reflect.provide(
            &self.ctx,
            GOAL_COMPLETION_GUARD_SERVICE,
            Some(cordis::arc(guard)),
            None,
        );
    }
    async fn dispose(self) {
        (self.detach)().await;
        self.ctx.fiber.dispose().await;
    }
}
fn reference(view: &GoalView) -> GoalRef {
    GoalRef {
        id: view.id.clone(),
        revision: view.revision,
    }
}

struct EvidenceGuard {
    bound: Arc<Mutex<GoalRequirementsIdentity>>,
    fresh: Arc<AtomicBool>,
    check_error: Arc<Mutex<Option<GoalCompletionError>>>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    wait: bool,
    prepare_error: Arc<Mutex<Option<GoalCompletionError>>>,
    cancel_on_check: bool,
    dropped: Arc<AtomicUsize>,
    goals: std::sync::Weak<GoalService>,
}
struct Permit {
    bound: Arc<Mutex<GoalRequirementsIdentity>>,
    fresh: Arc<AtomicBool>,
    check_error: Arc<Mutex<Option<GoalCompletionError>>>,
    cancel_on_check: bool,
    dropped: Arc<AtomicUsize>,
    goals: std::sync::Weak<GoalService>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
struct CommitGuard;
impl GoalCompletionCommitGuard for CommitGuard {}

impl GoalCompletionPermit for Permit {
    fn check<'a>(
        &'a self,
        agent: &Arc<dyn Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionCommitGuard + 'a>, GoalCompletionError> {
        // Reentrant reads must remain possible: no Goal cache mutex may be held.
        assert_eq!(
            self.goals
                .upgrade()
                .unwrap()
                .requirements_identity(agent)
                .unwrap()
                .as_ref(),
            Some(identity)
        );
        if self.cancel_on_check {
            agent.cancel(AgentCancelCause::User, None);
        }
        if let Some(error) = self.check_error.lock().clone() {
            return Err(error);
        }
        if !self.fresh.load(Ordering::SeqCst) || *self.bound.lock() != *identity {
            return Err("acceptance does not match current requirements or files".into());
        }
        Ok(Box::new(CommitGuard))
    }
}
#[async_trait::async_trait]
impl GoalCompletionGuard for EvidenceGuard {
    async fn prepare(
        &self,
        agent: &Arc<dyn Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionPermit>, GoalCompletionError> {
        assert_eq!(
            self.goals
                .upgrade()
                .unwrap()
                .requirements_identity(agent)
                .unwrap()
                .as_ref(),
            Some(identity)
        );
        if *self.bound.lock() != *identity {
            return Err("acceptance belongs to an older objective".into());
        }
        self.entered.notify_one();
        if self.wait {
            self.release.notified().await;
        }
        if let Some(error) = self.prepare_error.lock().clone() {
            return Err(error);
        }
        Ok(Box::new(Permit {
            bound: self.bound.clone(),
            fresh: self.fresh.clone(),
            check_error: self.check_error.clone(),
            cancel_on_check: self.cancel_on_check,
            dropped: self.dropped.clone(),
            goals: self.goals.clone(),
        }))
    }
}
fn guard(fixture: &Fixture, wait: bool, cancel_on_check: bool) -> Arc<EvidenceGuard> {
    Arc::new(EvidenceGuard {
        bound: Arc::new(Mutex::new(fixture.identity())),
        fresh: Arc::new(AtomicBool::new(true)),
        prepare_error: Arc::new(Mutex::new(None)),
        check_error: Arc::new(Mutex::new(None)),
        entered: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        wait,
        cancel_on_check,
        dropped: Arc::new(AtomicUsize::new(0)),
        goals: Arc::downgrade(&fixture.goals),
    })
}

#[tokio::test]
async fn requirements_identity_changes_only_with_objective_and_survives_cold_replay() {
    let f = Fixture::new().await;
    let first = f.create("Objective A");
    let identity = f.identity();
    let paused = f.goals.pause(&f.agent, &reference(&first)).unwrap();
    let resumed = f.goals.resume(&f.agent, &reference(&paused)).unwrap();
    let budget = f
        .goals
        .edit(
            &f.agent,
            &reference(&resumed),
            &EditGoalRequest {
                objective: Some(" Objective A ".into()),
                max_goal_rounds: Some(12),
            },
        )
        .unwrap();
    assert_eq!(f.identity(), identity);
    f.agent.session().append("user/message", json!({"id":"goal-round-1","role":"user","content":[],"source":{"kind":"goal","goalId":budget.id.as_str(),"revision":budget.revision,"round":1}}), Some(SurfaceIntent { surface_op: SurfaceOp::Append, source_event_seqs: None })).unwrap();
    assert_eq!(f.identity(), identity);
    assert_eq!(f.goals.get(&f.agent).unwrap().unwrap().rounds_started, 1);
    let edited = f
        .goals
        .edit(
            &f.agent,
            &reference(&budget),
            &EditGoalRequest {
                objective: Some("Objective B".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    assert_ne!(f.identity(), identity);
    assert_eq!(f.identity().objective_revision, edited.revision);
    let returned = f
        .goals
        .edit(
            &f.agent,
            &reference(&edited),
            &EditGoalRequest {
                objective: Some("Objective A".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    assert_ne!(
        f.identity(),
        identity,
        "A→B→A is a new requirements generation"
    );
    assert_eq!(f.identity().objective_revision, returned.revision);
    let events = f.agent.session().events();
    let cold = Session::create(
        f.agent.id().clone(),
        Some(events.to_vec()),
        Some(f.agent.session().header()),
        None,
    )
    .unwrap();
    assert_eq!(
        f.goals.requirements_identity_for_session(&cold),
        Some(f.identity())
    );
    let mut projection = Value::Null;
    for event in events.iter() {
        if let Some(next) = apply_goal_requirements_projection(&projection, event) {
            projection = next;
        }
    }
    assert_eq!(
        projection["goal"]["objectiveRevision"].as_u64(),
        Some(returned.revision)
    );
    assert_eq!(projection["goal"]["objective"], "Objective A");
    assert!(serde_json::to_vec(&projection).unwrap().len() < 2048);
    f.dispose().await;
}

#[tokio::test]
async fn requirements_lease_blocks_edits_without_holding_read_mutexes() {
    let f = Fixture::new().await;
    let goal = f.create("Stable objective");
    let lease = f.goals.claim_requirements(&f.agent).unwrap();
    assert_eq!(lease.identity(), Some(&f.identity()));
    assert_eq!(
        f.goals
            .edit(
                &f.agent,
                &reference(&goal),
                &EditGoalRequest {
                    objective: Some("Changed".into()),
                    max_goal_rounds: None
                }
            )
            .unwrap_err()
            .code,
        GoalErrorCode::CommitFailed
    );
    drop(lease);
    f.goals
        .edit(
            &f.agent,
            &reference(&goal),
            &EditGoalRequest {
                objective: Some("Changed".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    f.dispose().await;
}

#[tokio::test]
async fn completed_requirements_are_immutable_and_new_goal_never_reuses_the_identity() {
    let f = Fixture::new().await;
    let original = f.create("A");
    let identity = f.identity();
    let completed = f
        .goals
        .complete(&f.agent, &reference(&original))
        .await
        .unwrap();
    let before = f.agent.session().seq();
    assert_eq!(
        f.goals
            .edit(
                &f.agent,
                &reference(&completed),
                &EditGoalRequest {
                    objective: Some("B".into()),
                    max_goal_rounds: None
                }
            )
            .unwrap_err()
            .code,
        GoalErrorCode::InvalidTransition
    );
    assert_eq!(f.agent.session().seq(), before);
    assert_eq!(f.goals.get(&f.agent).unwrap().unwrap().objective, "A");
    let metadata = f
        .goals
        .edit(
            &f.agent,
            &reference(&completed),
            &EditGoalRequest {
                objective: None,
                max_goal_rounds: Some(16),
            },
        )
        .unwrap();
    assert_eq!(metadata.phase, GoalPhase::Complete);
    assert_eq!(f.identity(), identity);
    let fresh = f.create("B");
    assert_ne!(fresh.id, original.id);
    assert_eq!(f.identity().objective_revision, 1);
    f.goals.clear(&f.agent, &reference(&fresh)).unwrap();
    assert!(f.goals.requirements_identity(&f.agent).unwrap().is_none());
    let last = f.create("A");
    assert_ne!(last.id, original.id);
    assert_ne!(f.identity(), identity);
    f.dispose().await;
}

#[tokio::test]
async fn old_acceptance_and_changed_output_cannot_complete_current_requirements() {
    let f = Fixture::new().await;
    let first = f.create("A");
    let proof = guard(&f, false, false);
    f.install_guard(proof.clone());
    let changed = f
        .goals
        .edit(
            &f.agent,
            &reference(&first),
            &EditGoalRequest {
                objective: Some("B".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    assert_eq!(
        f.goals
            .complete(&f.agent, &reference(&changed))
            .await
            .unwrap_err()
            .code,
        GoalErrorCode::CompletionBlocked
    );
    *proof.bound.lock() = f.identity();
    proof.fresh.store(false, Ordering::SeqCst);
    assert_eq!(
        f.goals
            .complete(&f.agent, &reference(&changed))
            .await
            .unwrap_err()
            .code,
        GoalErrorCode::CompletionBlocked
    );
    assert_eq!(
        f.goals.get(&f.agent).unwrap().unwrap().phase,
        GoalPhase::Active
    );
    proof.fresh.store(true, Ordering::SeqCst);
    assert_eq!(
        f.goals
            .complete(&f.agent, &reference(&changed))
            .await
            .unwrap()
            .phase,
        GoalPhase::Complete
    );
    assert_eq!(proof.dropped.load(Ordering::SeqCst), 2);
    f.dispose().await;
}

#[tokio::test]
async fn edit_clear_pause_and_stop_during_prepare_prevent_late_completion() {
    for operation in ["edit", "clear", "pause", "stop"] {
        let f = Fixture::new().await;
        let goal = f.create("A");
        let proof = guard(&f, true, false);
        f.install_guard(proof.clone());
        let reference = reference(&goal);
        let mut completion = Box::pin(f.goals.complete(&f.agent, &reference));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut completion)
                .await
                .is_err()
        );
        match operation {
            "edit" => {
                f.goals
                    .edit(
                        &f.agent,
                        &reference,
                        &EditGoalRequest {
                            objective: Some("B".into()),
                            max_goal_rounds: None,
                        },
                    )
                    .unwrap();
            }
            "clear" => {
                f.goals.clear(&f.agent, &reference).unwrap();
            }
            "pause" => {
                f.goals.pause(&f.agent, &reference).unwrap();
            }
            "stop" => f.agent.cancel(AgentCancelCause::User, None),
            _ => unreachable!(),
        }
        proof.release.notify_one();
        let error = completion.await.unwrap_err();
        assert!(matches!(
            error.code,
            GoalErrorCode::StaleRevision | GoalErrorCode::NotFound | GoalErrorCode::Cancelled
        ));
        assert!(
            !f.goals
                .get(&f.agent)
                .unwrap()
                .is_some_and(|goal| goal.phase == GoalPhase::Complete)
        );
        assert_eq!(proof.dropped.load(Ordering::SeqCst), 1);
        f.dispose().await;
    }
}

#[tokio::test]
async fn cancellation_from_final_permit_check_is_rechecked_before_commit() {
    let f = Fixture::new().await;
    let goal = f.create("A");
    let proof = guard(&f, false, true);
    f.install_guard(proof.clone());
    assert_eq!(
        f.goals
            .complete(&f.agent, &reference(&goal))
            .await
            .unwrap_err()
            .code,
        GoalErrorCode::Cancelled
    );
    assert_eq!(
        f.goals.get(&f.agent).unwrap().unwrap().phase,
        GoalPhase::Active
    );
    assert_eq!(proof.dropped.load(Ordering::SeqCst), 1);
    f.dispose().await;
}

#[tokio::test]
async fn direct_user_requirement_changes_require_idle_but_budget_changes_do_not() {
    let f = Fixture::new().await;
    f.busy.store(true, Ordering::SeqCst);
    assert_eq!(
        f.goals
            .create_for_user(
                &f.agent,
                CreateGoalRequest {
                    objective: "A".into(),
                    max_goal_rounds: None
                }
            )
            .unwrap_err()
            .code,
        GoalErrorCode::AgentBusy
    );
    assert!(f.goals.get(&f.agent).unwrap().is_none());
    f.busy.store(false, Ordering::SeqCst);
    let goal = f
        .goals
        .create_for_user(
            &f.agent,
            CreateGoalRequest {
                objective: "A".into(),
                max_goal_rounds: Some(8),
            },
        )
        .unwrap();
    let identity = f.identity();
    f.busy.store(true, Ordering::SeqCst);
    assert_eq!(
        f.goals
            .edit_for_user(
                &f.agent,
                &reference(&goal),
                &EditGoalRequest {
                    objective: Some("B".into()),
                    max_goal_rounds: None
                }
            )
            .unwrap_err()
            .code,
        GoalErrorCode::AgentBusy
    );
    let budget = f
        .goals
        .edit_for_user(
            &f.agent,
            &reference(&goal),
            &EditGoalRequest {
                objective: Some(" A ".into()),
                max_goal_rounds: Some(16),
            },
        )
        .unwrap();
    assert_eq!(f.identity(), identity);
    f.busy.store(false, Ordering::SeqCst);
    let edited = f
        .goals
        .edit_for_user(
            &f.agent,
            &reference(&budget),
            &EditGoalRequest {
                objective: Some("B".into()),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    assert_eq!(edited.objective, "B");
    assert_ne!(f.identity(), identity);
    let lease = f.goals.claim_requirements(&f.agent).unwrap();
    assert_eq!(lease.phase(), Some(GoalPhase::Active));
    drop(lease);
    let view = f
        .goals
        .requirements_view_for_session(f.agent.session())
        .unwrap();
    assert_eq!(view["goalId"], edited.id.as_str());
    assert_eq!(view["objective"], "B");
    assert_eq!(view.as_object().unwrap().len(), 3);
    f.dispose().await;
}

#[tokio::test]
async fn authoritative_stop_takes_priority_over_a_prepare_failure() {
    let f = Fixture::new().await;
    let goal = f.create("A");
    let proof = guard(&f, true, false);
    *proof.prepare_error.lock() = Some(GoalCompletionError::Blocked(
        "filesystem verification was interrupted".into(),
    ));
    f.install_guard(proof.clone());
    let reference = reference(&goal);
    let mut completion = Box::pin(f.goals.complete(&f.agent, &reference));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut completion)
            .await
            .is_err()
    );
    f.agent.cancel(AgentCancelCause::User, None);
    proof.release.notify_one();
    assert_eq!(completion.await.unwrap_err().code, GoalErrorCode::Cancelled);
    assert_eq!(
        f.goals.get(&f.agent).unwrap().unwrap().phase,
        GoalPhase::Active
    );
    f.dispose().await;
}

#[tokio::test]
async fn validation_cancel_is_typed_even_without_an_agent_generation_change() {
    for final_check in [false, true] {
        let f = Fixture::new().await;
        let goal = f.create("A");
        let proof = guard(&f, false, false);
        if final_check {
            *proof.check_error.lock() = Some(GoalCompletionError::Cancelled);
        } else {
            *proof.prepare_error.lock() = Some(GoalCompletionError::Cancelled);
        }
        f.install_guard(proof);
        let generation = f.agent.cancellation_generation();
        assert_eq!(
            f.goals
                .complete(&f.agent, &reference(&goal))
                .await
                .unwrap_err()
                .code,
            GoalErrorCode::Cancelled
        );
        assert_eq!(f.agent.cancellation_generation(), generation);
        assert_eq!(
            f.goals.get(&f.agent).unwrap().unwrap().phase,
            GoalPhase::Active
        );
        f.dispose().await;
    }
}

struct UserWorkControl {
    busy: AtomicBool,
    calls: AtomicUsize,
}
impl GoalUserControl for UserWorkControl {
    fn with_idle(
        &self,
        agent: &Arc<dyn Agent>,
        operation: &mut dyn FnMut() -> Result<(), GoalError>,
    ) -> Result<(), GoalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.busy.load(Ordering::SeqCst) {
            return Err(GoalError::new(
                "descendant is still working",
                GoalErrorCode::AgentBusy,
            ));
        }
        let _control = agent
            .try_idle_control()
            .map_err(|_| GoalError::new("agent busy", GoalErrorCode::AgentBusy))?;
        operation()
    }
}

#[tokio::test]
async fn user_create_edit_and_clear_share_the_host_work_boundary_but_pause_and_budget_do_not() {
    let f = Fixture::new().await;
    let original = f.create("A");
    let control = Arc::new(UserWorkControl {
        busy: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    });
    let provided: Arc<dyn GoalUserControl> = control.clone();
    f.ctx.reflect.provide(
        &f.ctx,
        GOAL_USER_CONTROL_SERVICE,
        Some(cordis::arc(provided)),
        None,
    );
    let seq = f.agent.session().seq();
    let edited = EditGoalRequest {
        objective: Some("B".into()),
        max_goal_rounds: None,
    };
    assert_eq!(
        f.goals
            .edit_for_user(&f.agent, &reference(&original), &edited)
            .unwrap_err()
            .code,
        GoalErrorCode::AgentBusy
    );
    assert_eq!(
        f.goals
            .clear_for_user(&f.agent, &reference(&original))
            .unwrap_err()
            .code,
        GoalErrorCode::AgentBusy
    );
    assert_eq!(
        f.goals
            .create_for_user(
                &f.agent,
                CreateGoalRequest {
                    objective: "B".into(),
                    max_goal_rounds: Some(8)
                }
            )
            .unwrap_err()
            .code,
        GoalErrorCode::AgentBusy
    );
    assert_eq!(f.agent.session().seq(), seq);
    let budget = f
        .goals
        .edit_for_user(
            &f.agent,
            &reference(&original),
            &EditGoalRequest {
                objective: None,
                max_goal_rounds: Some(16),
            },
        )
        .unwrap();
    let paused = f.goals.pause(&f.agent, &reference(&budget)).unwrap();
    assert_eq!(control.calls.load(Ordering::SeqCst), 3);
    control.busy.store(false, Ordering::SeqCst);
    let revised = f
        .goals
        .edit_for_user(&f.agent, &reference(&paused), &edited)
        .unwrap();
    assert_eq!(revised.objective, "B");
    f.goals
        .clear_for_user(&f.agent, &reference(&revised))
        .unwrap();
    assert!(f.goals.get(&f.agent).unwrap().is_none());
    assert_eq!(
        f.goals
            .create_for_user(
                &f.agent,
                CreateGoalRequest {
                    objective: "C".into(),
                    max_goal_rounds: Some(8)
                }
            )
            .unwrap()
            .objective,
        "C"
    );
    f.dispose().await;
}
