//! Rust port of the core `packages/goal/goal/tests/goal.spec.ts` behaviors:
//! boundary validation, create/edit/phase transitions, CAS refs, blocked
//! reasons, clears with tombstones, and the live `goal/changed` event.

use std::sync::Arc;

use cordis::{ArcValue, Context, FiberCore, Plugin, downcast};
use parking_lot::Mutex;

use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_goal::{
    Config, CreateGoalRequest, EditGoalRequest, GoalActivation, GoalBlockReason, GoalErrorCode,
    GoalPhase, GoalRef, GoalService,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};

struct NoopPlugin;

#[async_trait::async_trait]
impl Plugin for NoopPlugin {
    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), cordis::PluginError> {
        Ok(())
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl StubAgent {
    fn new(ctx: &Context, raw_id: &str) -> (Arc<dyn Agent>, Arc<FiberCore>) {
        let fiber = ctx.plugin(Arc::new(NoopPlugin), cordis::arc(()));
        let agent_ctx = fiber.ctx().expect("plugin ctx bound at load");
        let id = session_id(raw_id);
        let session = Session::create(id.clone(), None, None).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        let agent: Arc<dyn Agent> = Arc::new(Self {
            id,
            session,
            inbox,
            ctx: agent_ctx,
            scope_key: ScopeKey::new(),
        });
        (agent, fiber)
    }
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

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: dsh_agent::AgentCancelCause, _options: Option<&dsh_agent::CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

struct Harness {
    ctx: Context,
    agents: Arc<AgentRegistry>,
    goals: Arc<GoalService>,
}

async fn setup(config: Config) -> Harness {
    let ctx = Context::root();
    let agents = AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, config);
    Harness { ctx, agents, goals }
}

async fn register_agent(harness: &Harness, agent: &Arc<dyn Agent>) {
    harness.agents.register(&harness.ctx, agent.clone());
    let id = agent.id().clone();
    for _ in 0..10_000 {
        if harness.agents.get(&id).is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("agent never became live");
}

fn assert_code(error: &dsh_goal::GoalError, code: GoalErrorCode) {
    assert_eq!(error.code, code, "error: {error}");
}

fn create_request(objective: &str) -> CreateGoalRequest {
    CreateGoalRequest {
        objective: objective.to_string(),
        max_goal_rounds: Some(8),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_agents_that_are_not_live() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "ghost");
    let error = harness
        .goals
        .create(&agent, create_request("hi"))
        .err()
        .expect("not live");
    assert_code(&error, GoalErrorCode::AgentNotLive);
}

#[tokio::test(flavor = "current_thread")]
async fn creates_an_armed_active_goal_and_rejects_a_second() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;

    let view = harness
        .goals
        .create(&agent, create_request("  finish the port  "))
        .expect("create");
    assert_eq!(view.objective, "finish the port");
    assert_eq!(view.phase, GoalPhase::Active);
    assert_eq!(view.activation, GoalActivation::Armed);
    assert_eq!(view.revision, 1);
    assert_eq!(view.rounds_started, 0);
    assert_eq!(view.max_goal_rounds, 8);
    assert!(view.created_at <= view.updated_at);
    assert!(view.id.as_str().starts_with("goal-"));

    // The durable change committed to the session log.
    assert!(agent
        .session()
        .events()
        .iter()
        .any(|event| event.type_ == "goal/change"));

    let error = harness
        .goals
        .create(&agent, create_request("second"))
        .err()
        .expect("already exists");
    assert_code(&error, GoalErrorCode::AlreadyExists);
}

#[tokio::test(flavor = "current_thread")]
async fn edits_objective_and_cap_without_changing_phase() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;
    let created = harness.goals.create(&agent, create_request("original")).expect("create");
    let ref_ = GoalRef {
        id: created.id.clone(),
        revision: created.revision,
    };

    let edited = harness
        .goals
        .edit(
            &agent,
            &ref_,
            &EditGoalRequest {
                objective: Some("updated".to_string()),
                max_goal_rounds: Some(12),
            },
        )
        .expect("edit");
    assert_eq!(edited.objective, "updated");
    assert_eq!(edited.max_goal_rounds, 12);
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.phase, GoalPhase::Active);

    let error = harness
        .goals
        .edit(
            &agent,
            &GoalRef {
                id: edited.id.clone(),
                revision: edited.revision,
            },
            &EditGoalRequest::default(),
        )
        .err()
        .expect("empty edit");
    assert_code(&error, GoalErrorCode::InvalidEdit);
    // The stale revision is refused.
    let error = harness
        .goals
        .edit(
            &agent,
            &ref_,
            &EditGoalRequest {
                objective: Some("late".to_string()),
                max_goal_rounds: None,
            },
        )
        .err()
        .expect("stale");
    assert_code(&error, GoalErrorCode::StaleRevision);
}

#[tokio::test(flavor = "current_thread")]
async fn pauses_and_resumes_through_the_phase_ladder() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;
    let created = harness.goals.create(&agent, create_request("ladder")).expect("create");

    let paused = harness
        .goals
        .pause(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 1 },
        )
        .expect("pause");
    assert_eq!(paused.phase, GoalPhase::Paused);
    assert_eq!(paused.activation, GoalActivation::Disarmed);
    assert_eq!(paused.revision, 2);

    let resumed = harness
        .goals
        .resume(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 2 },
        )
        .expect("resume");
    assert_eq!(resumed.phase, GoalPhase::Active);
    assert_eq!(resumed.activation, GoalActivation::Armed);

    // Resuming an already-armed active goal is refused.
    let error = harness
        .goals
        .resume(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 3 },
        )
        .err()
        .expect("already armed");
    assert_code(&error, GoalErrorCode::InvalidTransition);

    // Pausing a paused goal is refused through its stale ref; pausing the
    // current active revision again is legal.
    let error = harness
        .goals
        .pause(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 2 },
        )
        .err()
        .expect("stale phase");
    assert_code(&error, GoalErrorCode::StaleRevision);
    let paused_again = harness
        .goals
        .pause(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 3 },
        )
        .expect("pause active again");
    assert_eq!(paused_again.phase, GoalPhase::Paused);
}

#[tokio::test(flavor = "current_thread")]
async fn completes_allows_replacement_and_blocks_with_reasons() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;

    let created = harness.goals.create(&agent, create_request("first")).expect("create");

    // A bad reason is refused at the boundary (while the goal is still
    // active at revision 1).
    let error = harness
        .goals
        .block(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 1 },
            &GoalBlockReason {
                code: "BadCode".to_string(),
                message: "x".to_string(),
            },
        )
        .err()
        .expect("bad reason");
    assert_code(&error, GoalErrorCode::InvalidBlockReason);

    let blocked = harness
        .goals
        .block(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 1 },
            &GoalBlockReason {
                code: "round-limit".to_string(),
                message: "hit the cap".to_string(),
            },
        )
        .expect("block");
    assert_eq!(blocked.phase, GoalPhase::Blocked);
    assert_eq!(
        blocked.blocked_reason,
        Some(GoalBlockReason {
            code: "round-limit".to_string(),
            message: "hit the cap".to_string(),
        })
    );
    assert_eq!(blocked.activation, GoalActivation::Disarmed);

    let completed = harness
        .goals
        .complete(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 2 },
        )
        .expect("complete");
    assert_eq!(completed.phase, GoalPhase::Complete);
    assert_eq!(completed.activation, GoalActivation::Disarmed);

    // A completed goal may be replaced.
    let replacement = harness.goals.create(&agent, create_request("second")).expect("replace");
    assert_eq!(replacement.revision, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn clears_with_a_tombstone_and_allows_a_fresh_create() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;
    let created = harness.goals.create(&agent, create_request("to clear")).expect("create");

    let tombstone = harness
        .goals
        .clear(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 1 },
        )
        .expect("clear");
    assert_eq!(tombstone.id, created.id);
    assert_eq!(tombstone.revision, 2);

    assert!(harness.goals.get(&agent).expect("get").is_none());
    // The durable tombstone is in the log.
    assert!(agent
        .session()
        .events()
        .iter()
        .any(|event| event.type_ == "goal/change"
            && event.data.get("operation").and_then(|v| v.as_str()) == Some("clear")));

    // Missing and stale refs fail loudly.
    let error = harness
        .goals
        .edit(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 2 },
            &EditGoalRequest {
                objective: Some("x".to_string()),
                max_goal_rounds: None,
            },
        )
        .err()
        .expect("no current goal");
    assert_code(&error, GoalErrorCode::NotFound);

    let fresh = harness.goals.create(&agent, create_request("fresh")).expect("fresh create");
    assert_eq!(fresh.revision, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn emits_goal_changed_after_each_durable_mutation() {
    let harness = setup(Config::default()).await;
    let (agent, _fiber) = StubAgent::new(&harness.ctx, "owner");
    register_agent(&harness, &agent).await;

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_listener = seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(
        move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let seen = seen_for_listener.clone();
            Box::pin(async move {
                if let Some(payload) = args
                    .first()
                    .and_then(|value| downcast::<dsh_goal::domain::GoalChangedPayload>(value))
                {
                    seen.lock().push(payload.change.operation.as_str().to_string());
                }
                None
            })
        },
    );
    harness
        .ctx
        .on("goal/changed", listener, cordis::EventOptions::default().global(true))
        .await;

    let created = harness.goals.create(&agent, create_request("events")).expect("create");
    let _ = harness
        .goals
        .complete(
            &agent,
            &GoalRef { id: created.id.clone(), revision: 1 },
        )
        .expect("complete");

    let seen = seen.lock().clone();
    assert!(seen.contains(&"create".to_string()), "{seen:?}");
    assert!(seen.contains(&"complete".to_string()), "{seen:?}");
}
