use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_commands::{CommandInvocation, CommandResult, CommandRuntime, command_id};
use dsh_goal::{Config as GoalConfig, GoalBlockReason, GoalRef, GoalService};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, SessionStore, session_id};

struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl StubAgent {
    fn boxed(raw_id: &str) -> Arc<dyn Agent> {
        let id = session_id(raw_id);
        let session = Session::create(id, None, None).expect("session");
        Self::with_session(session)
    }

    fn with_session(session: Session) -> Arc<dyn Agent> {
        let id = session.id().clone();
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: Context::root(),
            scope_key: ScopeKey::new(),
        })
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

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

struct Harness {
    _ctx: Context,
    commands: Arc<CommandRuntime>,
    goals: Arc<GoalService>,
    agent: Arc<dyn Agent>,
}

impl Harness {
    fn new() -> Self {
        let ctx = Context::root();
        let commands = CommandRuntime::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let goals = GoalService::install(&ctx, GoalConfig::default());
        let _producer = dsh_command_goal::apply(&ctx).expect("apply");
        let agent = StubAgent::boxed("command-goal");
        let _detach = agents.enter(agent.clone(), None).expect("enter agent");
        Self {
            _ctx: ctx,
            commands,
            goals,
            agent,
        }
    }

    async fn run(&self, suffix: &str) -> CommandResult {
        self.commands
            .execute(&self.agent, &format!("/goal{suffix}"), never_abort())
            .await
            .expect("execute")
            .expect("resolved")
            .result
    }

    fn goal_change_count(&self) -> usize {
        self.agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn empty_goal_query_is_registered_and_does_not_mutate_goal_state() {
    let test = Harness::new();
    let descriptor = test
        .commands
        .list(&test.agent)
        .into_iter()
        .find(|descriptor| descriptor.name == "goal")
        .expect("registered /goal");
    assert_eq!(
        descriptor.description,
        "set or view the goal for a long-running task"
    );
    assert_eq!(
        descriptor.input.as_ref().map(|input| input.hint.as_str()),
        Some("[<objective>|clear|edit <objective>|pause|resume]")
    );

    let execution = test
        .commands
        .execute(&test.agent, "/goal", never_abort())
        .await
        .expect("execute")
        .expect("resolved");
    assert_eq!(
        execution.result,
        CommandResult::Success {
            text: Some(
                "No goal is currently set.\nUsage: /goal [<objective>|clear|edit <objective>|pause|resume]"
                    .to_string()
            ),
            source_event_seq: None,
        }
    );
    let event_types: Vec<String> = test
        .agent
        .session()
        .events()
        .iter()
        .map(|event| event.type_.clone())
        .collect();
    assert_eq!(event_types, vec!["command/run", "command/done"]);
}

#[tokio::test(flavor = "current_thread")]
async fn creates_a_trimmed_goal_and_refuses_silent_replacement() {
    let test = Harness::new();
    let created = test.run("\n  finish the release  ").await;
    let CommandResult::Success {
        text: Some(created),
        ..
    } = created
    else {
        panic!("expected goal creation success");
    };
    assert!(
        created.contains("Goal created\nStatus: active"),
        "{created}"
    );
    assert!(
        created.contains("Objective: finish the release"),
        "{created}"
    );
    assert!(created.contains("Rounds: 0/256"), "{created}");
    assert!(created.contains("Activation: armed"), "{created}");
    assert_eq!(test.goal_change_count(), 1);

    assert_eq!(
        test.run(" replacement").await,
        CommandResult::Error {
            text: "A goal is already active. Use /goal edit <objective> to change it or /goal clear before replacing it."
                .to_string(),
        }
    );
    assert_eq!(test.goal_change_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn edit_requires_a_replacement_and_a_current_goal() {
    let test = Harness::new();
    assert_eq!(
        test.run(" edit").await,
        CommandResult::Error {
            text: format!(
                "Goal editing requires a replacement objective.\n{}",
                "Usage: /goal [<objective>|clear|edit <objective>|pause|resume]"
            ),
        }
    );
    assert_eq!(
        test.run(" edit replacement").await,
        CommandResult::Error {
            text: "No goal is currently set; /goal edit requires one. Usage: /goal [<objective>|clear|edit <objective>|pause|resume]"
                .to_string(),
        }
    );
    assert!(test.goals.get(&test.agent).expect("goal read").is_none());
    assert_eq!(test.goal_change_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn edit_updates_the_exact_current_goal_revision() {
    let test = Harness::new();
    let _ = test.run(" first objective").await;
    let before = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("created goal");

    let updated = test.run(" EDIT\n  second objective  ").await;
    let CommandResult::Success {
        text: Some(updated),
        ..
    } = updated
    else {
        panic!("expected goal update success");
    };
    assert!(
        updated.contains("Goal updated\nStatus: active"),
        "{updated}"
    );
    assert!(updated.contains("Objective: second objective"), "{updated}");
    let after = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("updated goal");
    assert_eq!(after.id, before.id);
    assert_eq!(after.revision, 2);
    assert_eq!(after.objective, "second objective");
    assert_eq!(test.goal_change_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn pause_resume_and_clear_have_direct_missing_state_results() {
    let test = Harness::new();
    assert_eq!(
        test.run(" pause").await,
        CommandResult::Error {
            text: "No goal is currently set; /goal pause requires one. Usage: /goal [<objective>|clear|edit <objective>|pause|resume]"
                .to_string(),
        }
    );
    assert_eq!(
        test.run(" resume").await,
        CommandResult::Error {
            text: "No goal is currently set; /goal resume requires one. Usage: /goal [<objective>|clear|edit <objective>|pause|resume]"
                .to_string(),
        }
    );
    assert_eq!(
        test.run(" clear").await,
        CommandResult::Success {
            text: Some("No goal to clear.".to_string()),
            source_event_seq: None,
        }
    );
    assert_eq!(test.goal_change_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn pause_resume_and_clear_drive_the_durable_goal_lifecycle() {
    let test = Harness::new();
    let _ = test.run(" ship safely").await;

    let paused = test.run(" PAUSE").await;
    let CommandResult::Success {
        text: Some(paused), ..
    } = paused
    else {
        panic!("expected pause success");
    };
    assert!(paused.contains("Goal paused\nStatus: paused"), "{paused}");
    let goal = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("paused goal");
    assert_eq!(goal.revision, 2);
    assert_eq!(goal.phase.as_str(), "paused");

    let resumed = test.run(" resume").await;
    let CommandResult::Success {
        text: Some(resumed),
        ..
    } = resumed
    else {
        panic!("expected resume success");
    };
    assert!(
        resumed.contains("Goal resumed\nStatus: active"),
        "{resumed}"
    );
    assert!(resumed.contains("Activation: armed"), "{resumed}");
    let goal = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("resumed goal");
    assert_eq!(goal.revision, 3);

    assert_eq!(
        test.run(" clear").await,
        CommandResult::Success {
            text: Some("Goal cleared.".to_string()),
            source_event_seq: None,
        }
    );
    assert!(test.goals.get(&test.agent).expect("goal read").is_none());
    assert_eq!(test.goal_change_count(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn non_ascii_objectives_are_never_sliced_as_control_words() {
    let test = Harness::new();
    let created = test.run(" 编辑发布计划").await;
    let CommandResult::Success {
        text: Some(created),
        ..
    } = created
    else {
        panic!("expected unicode goal creation success");
    };
    assert!(created.contains("Objective: 编辑发布计划"), "{created}");
    assert_eq!(test.goal_change_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn only_exact_control_words_are_controls() {
    let test = Harness::new();
    let _ = test.run(" pause everything only after verification").await;
    let goal = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("created goal");
    assert_eq!(goal.objective, "pause everything only after verification");
}

#[tokio::test(flavor = "current_thread")]
async fn disarmed_active_state_offers_resume_instead_of_pause() {
    let test = Harness::new();
    let _ = test.run(" resumable work").await;
    test.goals.disarm(&test.agent).expect("disarm");
    let CommandResult::Success {
        text: Some(rendered),
        ..
    } = test.run("").await
    else {
        panic!("show succeeds");
    };
    assert!(rendered.contains("Status: active"), "{rendered}");
    assert!(rendered.contains("Activation: disarmed"), "{rendered}");
    assert!(rendered.contains("/goal resume"), "{rendered}");
}

#[tokio::test(flavor = "current_thread")]
async fn expected_goal_domain_rejections_become_human_command_errors() {
    let test = Harness::new();
    let _ = test.run(" already active").await;
    let before = test.goal_change_count();

    let execution = test
        .commands
        .execute(&test.agent, "/goal resume", never_abort())
        .await
        .expect("domain rejection must settle as a command result")
        .expect("resolved command");
    assert_eq!(
        execution.result,
        CommandResult::Error {
            text: "The goal command is not valid for the current state. Run /goal to view available commands."
                .to_string(),
        }
    );
    assert_eq!(test.goal_change_count(), before);
}

#[tokio::test(flavor = "current_thread")]
async fn cordis_plugin_and_invariant_companion_expose_the_package_contract() {
    let ctx = Context::root();
    let commands = CommandRuntime::install(&ctx);
    AgentRegistry::install(&ctx);
    GoalService::install(&ctx, GoalConfig::default());
    let agent = StubAgent::boxed("command-goal-plugin");

    let fiber = ctx.plugin(
        Arc::new(dsh_command_goal::CommandGoalPlugin),
        cordis::arc(()),
    );
    fiber.settle().await.expect("command-goal plugin settles");
    assert!(commands.find(&agent, "goal").is_some());

    assert_eq!(
        dsh_command_goal::invariant::PACKAGE_NAME,
        "@deepseek-ai/dsh-command-goal"
    );
    assert_eq!(dsh_command_goal::invariant::NAME, "command-goal-invariant");
    assert_eq!(dsh_command_goal::invariant::INJECT, ["invariants"]);
    assert!(dsh_command_goal::invariant::installer().inject.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn invariant_plugin_releases_its_package_reservation_on_dispose() {
    let ctx = Context::root();
    InvariantRegistry::new(&ctx, InvariantConfig::default());
    let fiber = ctx.plugin(
        Arc::new(dsh_command_goal::invariant::CommandGoalInvariantPlugin),
        cordis::arc(()),
    );
    fiber.settle().await.expect("invariant plugin settles");

    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_command_goal::invariant::apply(&ctx)
    }));
    assert!(
        duplicate.is_err(),
        "package is reserved while plugin is live"
    );

    fiber.dispose().await;
    let disposer = dsh_command_goal::invariant::apply(&ctx);
    disposer().await;
}

#[tokio::test(flavor = "current_thread")]
async fn show_renders_blocked_and_complete_states_with_exact_hints() {
    let test = Harness::new();
    let _ = test.run(" resolve provider outage").await;
    let current = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("current goal");
    let blocked = test
        .goals
        .block(
            &test.agent,
            &GoalRef {
                id: current.id,
                revision: current.revision,
            },
            &GoalBlockReason {
                code: "upstream-unavailable".to_string(),
                message: "Provider unavailable".to_string(),
            },
        )
        .expect("block");

    let CommandResult::Success {
        text: Some(rendered),
        ..
    } = test.run("").await
    else {
        panic!("blocked show succeeds");
    };
    assert!(rendered.contains("Status: blocked"), "{rendered}");
    assert!(
        rendered.contains("Blocker: upstream-unavailable: Provider unavailable"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Commands: /goal edit <objective>, /goal resume, /goal clear"),
        "{rendered}"
    );

    let resumed = test
        .goals
        .resume(
            &test.agent,
            &GoalRef {
                id: blocked.id,
                revision: blocked.revision,
            },
        )
        .expect("resume");
    test.goals
        .complete(
            &test.agent,
            &GoalRef {
                id: resumed.id,
                revision: resumed.revision,
            },
        )
        .expect("complete");
    let CommandResult::Success {
        text: Some(rendered),
        ..
    } = test.run("").await
    else {
        panic!("complete show succeeds");
    };
    assert!(rendered.contains("Status: complete"), "{rendered}");
    assert!(
        rendered.contains("Commands: /goal <objective>, /goal clear"),
        "{rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn editing_a_complete_goal_creates_a_fresh_identity() {
    let test = Harness::new();
    let _ = test.run(" first goal").await;
    let first = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("first goal");
    test.goals
        .complete(
            &test.agent,
            &GoalRef {
                id: first.id.clone(),
                revision: first.revision,
            },
        )
        .expect("complete");

    let replacement = test.run(" edit third goal").await;
    let CommandResult::Success {
        text: Some(replacement),
        ..
    } = replacement
    else {
        panic!("replacement creates");
    };
    assert!(replacement.starts_with("Goal created\n"), "{replacement}");
    let current = test
        .goals
        .get(&test.agent)
        .expect("goal read")
        .expect("replacement goal");
    assert_ne!(current.id, first.id);
    assert_eq!(current.revision, 1);
    assert_eq!(current.objective, "third goal");
}

#[tokio::test(flavor = "current_thread")]
async fn direct_disposer_and_plugin_disposal_remove_the_command() {
    let ctx = Context::root();
    let commands = CommandRuntime::install(&ctx);
    GoalService::install(&ctx, GoalConfig::default());
    let agent = StubAgent::boxed("command-goal-dispose");

    let disposer = dsh_command_goal::apply(&ctx).expect("apply");
    assert!(commands.find(&agent, "goal").is_some());
    disposer().await;
    assert!(commands.find(&agent, "goal").is_none());

    let fiber = ctx.plugin(
        Arc::new(dsh_command_goal::CommandGoalPlugin),
        cordis::arc(()),
    );
    fiber.settle().await.expect("plugin settles");
    assert!(commands.find(&agent, "goal").is_some());
    fiber.dispose().await;
    assert!(commands.find(&agent, "goal").is_none());
}

#[test]
fn apply_fails_loudly_when_either_required_service_is_missing() {
    let missing_commands = Context::root();
    GoalService::install(&missing_commands, GoalConfig::default());
    let commands_error = match dsh_command_goal::apply(&missing_commands) {
        Ok(_) => panic!("commands service is required"),
        Err(error) => error,
    };
    assert_eq!(commands_error, "command-goal requires the commands service");

    let missing_goals = Context::root();
    CommandRuntime::install(&missing_goals);
    let goals_error = match dsh_command_goal::apply(&missing_goals) {
        Ok(_) => panic!("goals service is required"),
        Err(error) => error,
    };
    assert_eq!(goals_error, "command-goal requires the goals service");
}

#[tokio::test(flavor = "current_thread")]
async fn durable_commit_failures_escape_the_human_domain_error_mapping() {
    let ctx = Context::root();
    let commands = CommandRuntime::install(&ctx);
    let agents = AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, GoalConfig::default());
    dsh_command_goal::apply(&ctx).expect("apply");
    let sessions = SessionStore::install(&ctx);
    let session = sessions
        .create(&ctx, Some(session_id("command-goal-commit-failure")), None)
        .await
        .expect("attached session");
    let agent = StubAgent::with_session(session);
    agents.enter(agent.clone(), None).expect("enter agent");
    let definition = commands.find(&agent, "goal").expect("goal command");

    let handler_result: Arc<std::sync::Mutex<Option<Result<CommandResult, String>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let result_for_listener = handler_result.clone();
    let agent_for_listener = agent.clone();
    ctx.on(
        "session/event",
        Arc::new(move |_ctx, args| {
            let is_trigger = args
                .get(1)
                .and_then(|value| cordis::downcast::<dsh_session::SessionEvent>(value))
                .is_some_and(|event| event.type_ == "command-goal-test/trigger");
            if is_trigger {
                let invocation = CommandInvocation {
                    command_id: command_id("commit-failure"),
                    agent: agent_for_listener.clone(),
                    raw_input: " create durably".to_string(),
                    signal: never_abort(),
                };
                let result = futures::executor::block_on((definition.handler)(&invocation));
                *result_for_listener.lock().expect("result lock") = Some(result);
            }
            Box::pin(async { None })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;

    agent
        .session()
        .append("command-goal-test/trigger", serde_json::json!({}), None)
        .expect("outer append");
    let result = handler_result
        .lock()
        .expect("result lock")
        .take()
        .expect("handler attempted");
    let error = result.expect_err("commit failure must escape as handler error");
    assert!(
        error.contains("failed to append durable goal change"),
        "{error}"
    );
    assert!(goals.get(&agent).expect("goal read").is_none());
}
