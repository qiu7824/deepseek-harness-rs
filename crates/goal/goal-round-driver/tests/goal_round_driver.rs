use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_agent::{AgentOptions, AgentRegistry, AgentStatus, SessionStartSource};
use dsh_agent_loop::{AgentLoop, Config as LoopConfig};
use dsh_goal::{CreateGoalRequest, EditGoalRequest, GoalActivation, GoalPhase, GoalService};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk,
};
use dsh_session::{SessionStore, session_id};
use dsh_tools::{Config as ToolsConfig, ToolRuntime};

struct DriverPlugin;

#[async_trait::async_trait]
impl Plugin for DriverPlugin {
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["agents", "goals", "sessions"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        dsh_goal_round_driver::apply(ctx)
            .map(|_| ())
            .map_err(|error| PluginError::new(cordis::arc(error)))
    }
}

struct RecordingAdapter {
    calls: Arc<AtomicUsize>,
}

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        response_stream()
    }
}

fn response_stream() -> ChunkStream {
    Box::pin(futures::stream::iter(vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "made concrete progress".to_string(),
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ]))
}

struct MaxTokensAdapter {
    calls: Arc<AtomicUsize>,
}

impl LlmAdapter for MaxTokensAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
            reason: FinishReason::MaxTokens,
            replay_state: None,
        }]))
    }
}

struct ErrorAdapter {
    calls: Arc<AtomicUsize>,
}

impl LlmAdapter for ErrorAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: dsh_llm::LlmFailure {
                    message: "round failed".to_string(),
                    code: "ROUND_FAILED".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        }]))
    }
}

struct CancelThenResponseAdapter {
    calls: Arc<AtomicUsize>,
    entered: Arc<AtomicBool>,
}

impl LlmAdapter for CancelThenResponseAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            return response_stream();
        }
        let signal = options
            .signal
            .clone()
            .expect("agent-loop cancellation signal");
        let entered = self.entered.clone();
        Box::pin(futures::stream::unfold(
            (signal, entered),
            |(signal, entered)| async move {
                entered.store(true, Ordering::SeqCst);
                loop {
                    if signal() {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            },
        ))
    }
}

struct SettledThenHangingAdapter {
    calls: Arc<AtomicUsize>,
}

impl LlmAdapter for SettledThenHangingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return response_stream();
        }
        let signal = options
            .signal
            .clone()
            .expect("agent-loop cancellation signal");
        Box::pin(futures::stream::unfold(signal, |signal| async move {
            loop {
                if signal() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }))
    }
}

struct SignalAwareHangingAdapter {
    calls: Arc<AtomicUsize>,
}

impl LlmAdapter for SignalAwareHangingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let signal = options
            .signal
            .clone()
            .expect("agent-loop cancellation signal");
        Box::pin(futures::stream::unfold(signal, |signal| async move {
            loop {
                if signal() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }))
    }
}

struct RequestTextAdapter {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl LlmAdapter for RequestTextAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        let text = options
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("\n");
        self.requests.lock().expect("requests lock").push(text);
        response_stream()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn armed_goal_drives_one_real_round_then_stops_at_the_budget() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-first-slice"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "finish one verified round".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create armed goal");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let current = goals.get(&agent).expect("current goal").expect("goal");
            if current.phase == GoalPhase::Blocked {
                break current;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let current = goals.get(&agent).expect("current goal");
        panic!(
            "driver did not settle one round: goal={:?}, requests={}, events={:?}",
            current.as_ref().map(|goal| (
                goal.phase.as_str(),
                goal.rounds_started,
                format!("{:?}", goal.activation),
            )),
            calls.load(Ordering::SeqCst),
            agent
                .session()
                .events()
                .iter()
                .map(|event| event.type_.clone())
                .collect::<Vec<_>>()
        )
    });

    let current = goals.get(&agent).expect("current goal").expect("goal");
    assert_eq!(current.rounds_started, 1);
    assert_eq!(
        current
            .blocked_reason
            .as_ref()
            .map(|reason| reason.code.as_str()),
        Some("round-limit")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let rounds: Vec<u64> = agent
        .session()
        .events()
        .iter()
        .filter_map(|event| {
            if event.type_ != "user/message" {
                return None;
            }
            let source = event.data.get("source")?.as_object()?;
            (source.get("kind")?.as_str()? == "goal")
                .then(|| source.get("round")?.as_u64())
                .flatten()
        })
        .filter(|round| *round > 0)
        .collect();
    assert_eq!(rounds, vec![1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_positive_goal_round_never_reaches_the_model() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-forged"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "forged automatic work".to_string(),
        }],
        dsh_llm::MessageSource::Goal {
            goal_id: "forged-goal".to_string(),
            revision: 1,
            round: 1,
        },
    ));
    agent.when_idle().await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "an unreserved positive goal round must be rejected before model dispatch"
    );
    assert!(
        agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "turn/start"),
        "the loop may open a balanced turn before the pre-step veto"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_zero_goal_context_uses_the_ordinary_pre_step_chain() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-context"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "goal context only".to_string(),
        }],
        dsh_llm::MessageSource::Goal {
            goal_id: "context-goal".to_string(),
            revision: 1,
            round: 0,
        },
    ));
    agent.when_idle().await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hot_load_disarms_existing_goal_until_explicit_resume() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-hot-load"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    let created = goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "wait for explicit resume".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal before driver");

    let _driver = dsh_goal_round_driver::apply(&ctx).expect("hot-load driver");
    let current = goals.get(&agent).expect("goal read").expect("goal");
    assert_eq!(
        current.activation,
        GoalActivation::Disarmed,
        "a new producer must not inherit hidden automatic authority"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    goals
        .resume(
            &agent,
            &dsh_goal::GoalRef {
                id: created.id,
                revision: created.revision,
            },
        )
        .expect("explicit resume");
    let blocked = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("explicit resume should drive the goal");
    assert_eq!(blocked.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_start_resets_process_local_attempt_until_explicit_resume() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let emitted_for_listener = emitted.clone();
    let ctx_for_listener = ctx.clone();
    ctx.on(
        "agent/inbox/inserted",
        Arc::new(move |_ctx, args| {
            let emitted = emitted_for_listener.clone();
            let root = ctx_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxMessagePayload>())
                    .cloned()
                    .expect("inserted payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !emitted.swap(true, Ordering::SeqCst)
                {
                    dsh_agent::emit_agent_event(
                        &root,
                        &payload.agent,
                        "agent/session-start",
                        |agent| {
                            cordis::arc(dsh_agent::AgentSessionStartPayload {
                                agent: agent.clone(),
                                source: SessionStartSource::Resume,
                            })
                        },
                    );
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-session-start-reset"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    let created = goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "restart safely".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");
    let reset = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if emitted.load(Ordering::SeqCst) && goal.activation == GoalActivation::Disarmed {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session-start edge should disarm the goal");
    agent.when_idle().await;
    assert_eq!(reset.rounds_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    goals
        .resume(
            &agent,
            &dsh_goal::GoalRef {
                id: created.id,
                revision: created.revision,
            },
        )
        .expect("explicit resume");
    let blocked = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("resumed goal should drive once");
    assert_eq!(blocked.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caller_fiber_disposal_runs_the_composite_driver_teardown() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let fiber = ctx.plugin(Arc::new(DriverPlugin), cordis::arc(()));
    fiber.settle().await.expect("driver plugin");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SignalAwareHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-fiber-dispose"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "dispose the owning fiber".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal round must reach the model");

    tokio::time::timeout(Duration::from_secs(1), fiber.dispose())
        .await
        .expect("caller fiber teardown");
    let status_after_dispose = agent.status();
    let goal_after_dispose = goals.get(&agent).expect("goal read").expect("goal");
    if status_after_dispose == AgentStatus::Running {
        agent.cancel(dsh_agent::AgentCancelCause::User, None);
        agent.when_idle().await;
    }

    assert_eq!(status_after_dispose, AgentStatus::Idle);
    assert_eq!(goal_after_dispose.activation, GoalActivation::Disarmed);
    assert_eq!(goal_after_dispose.rounds_started, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teardown_waits_for_an_inflight_driver_checkpoint() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let checkpoint_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checkpoint_started_listener = checkpoint_started.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_listener = release.clone();
    ctx.on(
        "session/flush",
        Arc::new(move |_ctx, _args| {
            let started = checkpoint_started_listener.clone();
            let release = release_listener.clone();
            Box::pin(async move {
                started.store(true, Ordering::SeqCst);
                release.notified().await;
                None
            })
        }),
        cordis::EventOptions::default(),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-teardown-checkpoint"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "wait for the checkpoint".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !checkpoint_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("driver checkpoint must start");

    let mut teardown = tokio::spawn(driver());
    if tokio::time::timeout(Duration::from_millis(50), &mut teardown)
        .await
        .is_ok()
    {
        release.notify_one();
        panic!("driver teardown returned before its checkpoint task settled");
    }
    release.notify_one();
    teardown.await.expect("teardown task");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let goal = goals.get(&agent).expect("goal read").expect("goal");
    assert_eq!(goal.activation, GoalActivation::Disarmed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_goal_attempt_does_not_claim_a_later_human_turn_during_teardown() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SettledThenHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-settled-attempt-teardown"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "finish before human work".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if goals
                .get(&agent)
                .expect("goal read")
                .is_some_and(|goal| goal.phase == GoalPhase::Blocked)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal round must settle");

    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "later ordinary human work".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("later human turn must reach the model");

    tokio::time::timeout(Duration::from_millis(100), driver())
        .await
        .expect("teardown must not wait for later human work");
    assert_eq!(
        agent.status(),
        AgentStatus::Running,
        "a settled Goal attempt must not retain ownership of a later human turn"
    );

    agent.cancel(dsh_agent::AgentCancelCause::User, None);
    agent.when_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teardown_removes_a_stale_queued_round_without_cancelling_human_work() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SignalAwareHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let inserted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let inserted_for_listener = inserted.clone();
    ctx.on(
        "agent/inbox/inserted",
        Arc::new(move |_ctx, args| {
            let inserted = inserted_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxMessagePayload>())
                    .cloned()
                    .expect("inserted payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !inserted.swap(true, Ordering::SeqCst)
                {
                    payload.agent.followup(dsh_llm::create_user_message(
                        vec![ContentBlock::Text {
                            text: "human work survives stale teardown".to_string(),
                        }],
                        dsh_llm::MessageSource::User {
                            rpc_id: None,
                            client_time_zone: None,
                        },
                    ));
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let teardown_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let teardown_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_for_status = teardown_started.clone();
    let done_for_status = teardown_done.clone();
    let driver_for_status = driver.clone();
    ctx.on(
        "agent/status",
        Arc::new(move |_ctx, args| {
            let started = started_for_status.clone();
            let done = done_for_status.clone();
            let driver = driver_for_status.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentStatusPayload>())
                    .expect("status payload");
                if payload.status == AgentStatus::Running && !started.swap(true, Ordering::SeqCst) {
                    tokio::spawn(async move {
                        driver().await;
                        done.store(true, Ordering::SeqCst);
                    });
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-stale-human-teardown"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "yield during teardown".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !teardown_done.load(Ordering::SeqCst) || calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "stale teardown stalled: done={}, status={:?}, calls={}, next_turn={:?}, events={:?}, goal={:?}",
            teardown_done.load(Ordering::SeqCst),
            agent.status(),
            calls.load(Ordering::SeqCst),
            agent
                .inbox()
                .next_turn()
                .iter()
                .map(|message| message.source.clone())
                .collect::<Vec<_>>(),
            agent
                .session()
                .events()
                .iter()
                .map(|event| (event.type_.clone(), event.data.clone()))
                .collect::<Vec<_>>(),
            goals.get(&agent),
        )
    });

    assert_eq!(agent.status(), AgentStatus::Running);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    agent.cancel(dsh_agent::AgentCancelCause::User, None);
    agent.when_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teardown_does_not_cancel_an_unrelated_human_turn() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    GoalService::install(&ctx, Default::default());
    let driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SignalAwareHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-unrelated-human-teardown"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "ordinary human work".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ordinary turn must reach the model");

    tokio::time::timeout(Duration::from_millis(100), driver())
        .await
        .expect("driver teardown must not wait for unrelated work");
    assert_eq!(
        agent.status(),
        AgentStatus::Running,
        "goal driver teardown must not cancel an unrelated human turn"
    );
    assert!(
        !agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "turn/end"),
        "unrelated human turn must remain live after driver teardown"
    );

    agent.cancel(dsh_agent::AgentCancelCause::User, None);
    agent.when_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teardown_cancels_an_active_round_and_waits_for_idle() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SignalAwareHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-teardown"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "survive driver teardown".to_string(),
                max_goal_rounds: Some(3),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal round should enter the model");

    tokio::time::timeout(Duration::from_secs(1), driver())
        .await
        .expect("driver teardown should cancel and drain its active round");
    tokio::time::timeout(Duration::from_millis(100), driver())
        .await
        .expect("driver teardown must be idempotent");

    let current = goals.get(&agent).expect("goal read").expect("goal");
    assert_eq!(current.phase, GoalPhase::Active);
    assert_eq!(current.activation, GoalActivation::Disarmed);
    assert_eq!(current.rounds_started, 1);
    let status_at_return = agent.status();
    if status_at_return != AgentStatus::Idle {
        let eventually_idle = tokio::time::timeout(Duration::from_millis(100), async {
            while agent.status() != AgentStatus::Idle {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        panic!(
            "teardown returned before idle: status_at_return={:?}, eventually_idle={}, events={:?}",
            status_at_return,
            eventually_idle,
            agent
                .session()
                .events()
                .iter()
                .map(|event| (event.type_.clone(), event.data.clone()))
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_initial_checkpoint_disarms_without_model_dispatch() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    ctx.on(
        "session/flush",
        Arc::new(|_ctx, _args| {
            Box::pin(async move {
                panic!("disk unavailable");
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-flush-failed"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "do not outrun storage".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.activation == GoalActivation::Disarmed {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("flush failure should disarm");

    assert_eq!(current.phase, GoalPhase::Active);
    assert_eq!(current.rounds_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_post_round_checkpoint_prevents_a_second_round() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let flushes = Arc::new(AtomicUsize::new(0));
    let flushes_for_listener = flushes.clone();
    ctx.on(
        "session/flush",
        Arc::new(move |_ctx, _args| {
            let flushes = flushes_for_listener.clone();
            Box::pin(async move {
                if flushes.fetch_add(1, Ordering::SeqCst) >= 1 {
                    panic!("round checkpoint failed");
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-second-flush-failed"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "no second round without durability".to_string(),
                max_goal_rounds: Some(3),
            },
        )
        .expect("create goal");
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.activation == GoalActivation::Disarmed {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second flush failure should disarm");

    assert_eq!(current.phase, GoalPhase::Active);
    assert_eq!(current.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(flushes.load(Ordering::SeqCst) >= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_tokens_disarms_without_opening_another_round() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(MaxTokensAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-max-tokens"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "stop after truncation".to_string(),
                max_goal_rounds: Some(3),
            },
        )
        .expect("create goal");
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.activation == GoalActivation::Disarmed {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let goal = goals.get(&agent).expect("goal read").expect("goal");
        panic!(
            "max-tokens did not disarm: phase={}, rounds={}, calls={}",
            goal.phase.as_str(),
            goal.rounds_started,
            calls.load(Ordering::SeqCst)
        )
    });

    assert_eq!(current.phase, GoalPhase::Active);
    assert_eq!(current.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_round_error_disarms_without_automatic_retry() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(ErrorAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-terminal-error"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "stop after terminal error".to_string(),
                max_goal_rounds: Some(3),
            },
        )
        .expect("create goal");
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.activation == GoalActivation::Disarmed {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let goal = goals.get(&agent).expect("goal read").expect("goal");
        panic!(
            "terminal error did not disarm: phase={}, rounds={}, calls={}",
            goal.phase.as_str(),
            goal.rounds_started,
            calls.load(Ordering::SeqCst)
        )
    });

    assert_eq!(current.phase, GoalPhase::Active);
    assert_eq!(current.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_unrelated_human_work_disarms_new_automatic_authority() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(AtomicBool::new(false));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(CancelThenResponseAdapter {
            calls: calls.clone(),
            entered: entered.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-cancel-unrelated"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");
    agent.followup(dsh_llm::create_user_message(
        vec![ContentBlock::Text {
            text: "ordinary human turn".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("human turn stream must be actively polling");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "wait for explicit resume".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal during human turn");
    agent.cancel(dsh_agent::AgentCancelCause::User, None);
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if agent.status() == AgentStatus::Idle {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled human turn should settle");

    assert_eq!(current.activation, GoalActivation::Disarmed);
    assert_eq!(current.rounds_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarded_claimed_reservation_never_reaches_the_model() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let discarded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let discarded_for_listener = discarded.clone();
    let root = ctx.clone();
    ctx.on(
        "agent/inbox/claimed",
        Arc::new(move |_ctx, args| {
            let discarded = discarded_for_listener.clone();
            let root = root.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxClaimedPayload>())
                    .cloned()
                    .expect("claimed payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !discarded.swap(true, Ordering::SeqCst)
                {
                    let message = payload.message.clone();
                    dsh_agent::emit_agent_event(
                        &root,
                        &payload.agent,
                        "agent/inbox/discarded",
                        move |agent| {
                            cordis::arc(dsh_agent::AgentInboxMessagePayload {
                                agent: agent.clone(),
                                message: message.clone(),
                            })
                        },
                    );
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-discarded-claimed"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "discard claimed authority".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    let paused = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Paused {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("discarded claimed reservation should pause");

    assert_eq!(paused.rounds_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_after_claim_pauses_without_admitting_the_round() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled_for_listener = cancelled.clone();
    ctx.on(
        "agent/inbox/claimed",
        Arc::new(move |_ctx, args| {
            let cancelled = cancelled_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxClaimedPayload>())
                    .cloned()
                    .expect("inbox claimed payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !cancelled.swap(true, Ordering::SeqCst)
                {
                    payload
                        .agent
                        .cancel(dsh_agent::AgentCancelCause::User, None);
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-cancel-before-step"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "do not start yet".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");

    let paused = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Paused {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let goal = goals.get(&agent).expect("goal read").expect("goal");
        panic!(
            "cancelled reservation did not pause: phase={}, rounds={}, activation={:?}, requests={}, events={:?}",
            goal.phase.as_str(),
            goal.rounds_started,
            goal.activation,
            calls.load(Ordering::SeqCst),
            agent
                .session()
                .events()
                .iter()
                .map(|event| event.type_.clone())
                .collect::<Vec<_>>()
        )
    });

    assert_eq!(paused.rounds_started, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!agent.session().events().iter().any(|event| {
        event.type_ == "user/message"
            && event.data["source"]["kind"] == "goal"
            && event.data["source"]["round"]
                .as_u64()
                .is_some_and(|round| round > 0)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_an_admitted_round_pauses_at_the_admitted_count() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(SignalAwareHangingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-cancel-active-step"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "stop in flight".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal model request should start");

    agent.cancel(dsh_agent::AgentCancelCause::User, None);
    let paused = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Paused {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("admitted cancelled round should pause");
    agent.when_idle().await;

    assert_eq!(paused.rounds_started, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_work_queued_behind_a_reserved_round_runs_first() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let requests = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RequestTextAdapter {
            requests: requests.clone(),
        }),
    )
    .expect("adapter");
    let inserted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let inserted_for_listener = inserted.clone();
    ctx.on(
        "agent/inbox/inserted",
        Arc::new(move |_ctx, args| {
            let inserted = inserted_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxMessagePayload>())
                    .cloned()
                    .expect("inbox inserted payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !inserted.swap(true, Ordering::SeqCst)
                {
                    payload.agent.followup(dsh_llm::create_user_message(
                        vec![ContentBlock::Text {
                            text: "human joined the pending batch".to_string(),
                        }],
                        dsh_llm::MessageSource::User {
                            rpc_id: None,
                            client_time_zone: None,
                        },
                    ));
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-human-priority"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "yield to human work".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal should settle after the human and goal turns");

    let requests = requests.lock().expect("requests lock").clone();
    assert_eq!(
        requests.len(),
        2,
        "expected one human turn and one goal round"
    );
    assert!(requests[0].contains("human joined the pending batch"));
    assert!(
        !requests[0].contains("<goal_round>"),
        "the stale automatic reservation must not run before human work"
    );
    assert!(requests[1].contains("<goal_round>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_goal_rejection_restores_other_claimed_step_context() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let requests = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RequestTextAdapter {
            requests: requests.clone(),
        }),
    )
    .expect("adapter");
    let staged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let staged_for_listener = staged.clone();
    let goals_for_listener = goals.clone();
    ctx.on(
        "agent/inbox/inserted",
        Arc::new(move |_ctx, args| {
            let staged = staged_for_listener.clone();
            let goals = goals_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxMessagePayload>())
                    .cloned()
                    .expect("inserted payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !staged.swap(true, Ordering::SeqCst)
                {
                    payload.agent.inject(dsh_llm::create_user_message(
                        vec![ContentBlock::Text {
                            text: "claimed plugin context must survive".to_string(),
                        }],
                        dsh_llm::MessageSource::Plugin {
                            plugin: "test".to_string(),
                            form: None,
                            sections: None,
                            summary: None,
                            compaction_id: None,
                            source_command_id: None,
                        },
                    ));
                    let goal = goals.get(&payload.agent).expect("goal read").expect("goal");
                    goals
                        .edit(
                            &payload.agent,
                            &dsh_goal::GoalRef {
                                id: goal.id,
                                revision: goal.revision,
                            },
                            &EditGoalRequest {
                                objective: Some("new objective with restored context".to_string()),
                                max_goal_rounds: None,
                            },
                        )
                        .expect("edit queued goal");
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-restore-claimed"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "old objective".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if goals
                .get(&agent)
                .expect("goal read")
                .is_some_and(|goal| goal.phase == GoalPhase::Blocked)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("new revision should settle");

    let requests = requests.lock().expect("requests lock").clone();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].contains("claimed plugin context must survive"),
        "rejecting only the stale Goal reservation must restore other claimed messages"
    );
    assert!(!requests[0].contains("<goal_round>"));
    assert!(requests[1].contains("new objective with restored context"));
    assert!(requests[1].contains("<goal_round>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_goal_edit_discards_the_old_prompt_and_runs_the_new_revision() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let requests = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RequestTextAdapter {
            requests: requests.clone(),
        }),
    )
    .expect("adapter");
    let edited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let edited_for_listener = edited.clone();
    let goals_for_listener = goals.clone();
    ctx.on(
        "agent/inbox/inserted",
        Arc::new(move |_ctx, args| {
            let edited = edited_for_listener.clone();
            let goals = goals_for_listener.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentInboxMessagePayload>())
                    .cloned()
                    .expect("inserted payload");
                if matches!(
                    payload.message.source,
                    dsh_llm::MessageSource::Goal { round, .. } if round > 0
                ) && !edited.swap(true, Ordering::SeqCst)
                {
                    let goal = goals.get(&payload.agent).expect("goal read").expect("goal");
                    goals
                        .edit(
                            &payload.agent,
                            &dsh_goal::GoalRef {
                                id: goal.id,
                                revision: goal.revision,
                            },
                            &EditGoalRequest {
                                objective: Some("new objective".to_string()),
                                max_goal_rounds: None,
                            },
                        )
                        .expect("edit queued goal");
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-edit-queued"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "old objective".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");
    let blocked = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("edited goal should run one fresh round");

    assert_eq!(blocked.objective, "new objective");
    assert_eq!(blocked.revision, 3);
    assert_eq!(blocked.rounds_started, 1);
    let requests = requests.lock().expect("requests lock").clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("new objective"));
    assert!(!requests[0].contains("old objective"));
    let admitted_revision = agent
        .session()
        .events()
        .iter()
        .find(|event| {
            event.type_ == "user/message"
                && event.data["source"]["kind"] == "goal"
                && event.data["source"]["round"] == 1
        })
        .and_then(|event| event.data["source"]["revision"].as_u64());
    assert_eq!(admitted_revision, Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_rejection_does_not_block_a_new_goal_revision() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let goals_for_hook = goals.clone();
    let edited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let edited_for_hook = edited.clone();
    ctx.on(
        "agent/pre-step",
        Arc::new(move |_ctx, args| {
            let goals = goals_for_hook.clone();
            let edited = edited_for_hook.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                    .expect("pre-step payload");
                if payload.messages.iter().any(|message| {
                    matches!(message.source, dsh_llm::MessageSource::Goal { round, .. } if round > 0)
                }) && !edited.swap(true, Ordering::SeqCst)
                {
                    let goal = goals.get(&payload.agent).expect("goal read").expect("goal");
                    goals
                        .edit(
                            &payload.agent,
                            &dsh_goal::GoalRef {
                                id: goal.id,
                                revision: goal.revision,
                            },
                            &EditGoalRequest {
                                objective: Some("replacement objective".to_string()),
                                max_goal_rounds: None,
                            },
                        )
                        .expect("edit before rejection");
                    return Some(cordis::arc(dsh_agent::PreStepDecision::Reject));
                }
                let next = cordis::downcast_arc::<cordis::NextFn>(
                    args.last().expect("pre-step next"),
                )
                .expect("pre-step next");
                Some(next.call().await)
            })
        }),
        cordis::EventOptions::default(),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-reject-old-revision"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "old objective".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    let current = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("new revision should run and reach its round limit");

    assert_eq!(current.objective, "replacement objective");
    assert_eq!(current.phase, GoalPhase::Blocked);
    assert_eq!(current.rounds_started, 2);
    assert_eq!(
        current
            .blocked_reason
            .as_ref()
            .map(|reason| reason.code.as_str()),
        Some("round-limit"),
        "the old prompt rejection must not block the replacement revision"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_prompt_rejection_blocks_the_goal_without_model_dispatch() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    ctx.on(
        "agent/pre-step",
        Arc::new(move |_ctx, args| {
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                    .expect("pre-step payload");
                if payload.messages.iter().any(|message| {
                    matches!(message.source, dsh_llm::MessageSource::Goal { round, .. } if round > 0)
                }) {
                    return Some(cordis::arc(dsh_agent::PreStepDecision::Reject));
                }
                let next = cordis::downcast_arc::<cordis::NextFn>(
                    args.last().expect("pre-step next"),
                )
                .expect("pre-step next");
                Some(next.call().await)
            })
        }),
        cordis::EventOptions::default(),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-prompt-rejected"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "block rejected prompt".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");
    let blocked = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let goal = goals.get(&agent).expect("goal read").expect("goal");
        panic!(
            "prompt rejection did not block: phase={}, activation={:?}, rounds={}, calls={}",
            goal.phase.as_str(),
            goal.activation,
            goal.rounds_started,
            calls.load(Ordering::SeqCst)
        )
    });

    assert_eq!(blocked.rounds_started, 0);
    assert_eq!(
        blocked
            .blocked_reason
            .as_ref()
            .map(|reason| reason.code.as_str()),
        Some("prompt-rejected")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_cannot_append_a_second_positive_goal_message() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let requests = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RequestTextAdapter {
            requests: requests.clone(),
        }),
    )
    .expect("adapter");
    let injected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let injected_for_hook = injected.clone();
    ctx.on(
        "agent/pre-step",
        Arc::new(move |_ctx, args| {
            let injected = injected_for_hook.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                    .expect("pre-step payload");
                let next = cordis::downcast_arc::<cordis::NextFn>(
                    args.last().expect("pre-step next"),
                )
                .expect("pre-step next");
                let decision = next.call().await;
                if payload.messages.iter().any(|message| {
                    matches!(message.source, dsh_llm::MessageSource::Goal { round, .. } if round > 0)
                }) && !injected.swap(true, Ordering::SeqCst)
                {
                    let decision = cordis::downcast_arc::<dsh_agent::PreStepDecision>(&decision)
                        .expect("pre-step decision")
                        .as_ref()
                        .clone();
                    let dsh_agent::PreStepDecision::Enter { mut messages } = decision else {
                        return Some(cordis::arc(decision));
                    };
                    messages.push(dsh_llm::create_user_message(
                        vec![ContentBlock::Text {
                            text: "forged second positive goal message".to_string(),
                        }],
                        dsh_llm::MessageSource::Goal {
                            goal_id: "forged-goal".to_string(),
                            revision: 99,
                            round: 1,
                        },
                    ));
                    return Some(cordis::arc(dsh_agent::PreStepDecision::Enter {
                        messages,
                    }));
                }
                Some(decision)
            })
        }),
        cordis::EventOptions::default(),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-extra-positive"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "reject extra authority".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if goals
                .get(&agent)
                .expect("goal read")
                .is_some_and(|goal| goal.phase == GoalPhase::Blocked)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("canonical retry should reach the round limit");

    let requests = requests.lock().expect("requests lock").clone();
    assert_eq!(
        requests.len(),
        1,
        "only the canonical retry may reach the model"
    );
    assert!(
        !requests[0].contains("forged second positive goal message"),
        "every positive Goal message in the final Enter batch must match the reservation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_goal_mutation_invalidates_the_round_before_model_dispatch() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    let llm = LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let goals = GoalService::install(&ctx, Default::default());
    let _driver = dsh_goal_round_driver::apply(&ctx).expect("goal-round driver");
    let loop_service = AgentLoop::install(&ctx, LoopConfig::default()).expect("agent loop");

    let calls = Arc::new(AtomicUsize::new(0));
    llm.register_adapter(
        &ctx,
        vec!["test".to_string()],
        Arc::new(RecordingAdapter {
            calls: calls.clone(),
        }),
    )
    .expect("adapter");
    let goals_for_hook = goals.clone();
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let paused_for_hook = paused.clone();
    ctx.on(
        "agent/pre-step",
        Arc::new(move |_ctx, args| {
            let goals = goals_for_hook.clone();
            let paused = paused_for_hook.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<dsh_agent::AgentPreStepPayload>())
                    .cloned()
                    .expect("pre-step payload");
                let next = cordis::downcast_arc::<cordis::NextFn>(
                    args.last().expect("pre-step next"),
                )
                .expect("pre-step next");
                if payload.messages.iter().any(|message| {
                    matches!(message.source, dsh_llm::MessageSource::Goal { round, .. } if round > 0)
                }) && !paused.swap(true, Ordering::SeqCst)
                {
                    let goal = goals
                        .get(&payload.agent)
                        .expect("goal read")
                        .expect("goal");
                    goals
                        .pause(
                            &payload.agent,
                            &dsh_goal::GoalRef {
                                id: goal.id,
                                revision: goal.revision,
                            },
                        )
                        .expect("pause during downstream hook");
                }
                Some(next.call().await)
            })
        }),
        cordis::EventOptions::default(),
    )
    .await;
    let agent = loop_service
        .create(
            &session_id("goal-round-driver-post-hook"),
            &AgentOptions {
                provider: Some("test".to_string()),
                model: Some("model".to_string()),
                ..Default::default()
            },
            None,
        )
        .expect("agent");

    goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "do not run stale work".to_string(),
                max_goal_rounds: Some(2),
            },
        )
        .expect("create goal");

    let paused_goal = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let goal = goals.get(&agent).expect("goal read").expect("goal");
            if goal.phase == GoalPhase::Paused {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal should pause in downstream pre-step hook");
    agent.when_idle().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if agent
                .session()
                .events()
                .iter()
                .any(|event| event.type_ == "turn/end")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "turn did not close: requests={}, events={:?}",
            calls.load(Ordering::SeqCst),
            agent
                .session()
                .events()
                .iter()
                .map(|event| event.type_.clone())
                .collect::<Vec<_>>()
        )
    });

    assert_eq!(paused_goal.rounds_started, 0);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a downstream mutation must invalidate the reservation before model dispatch"
    );
    let positive_goal_messages = agent
        .session()
        .events()
        .iter()
        .filter(|event| {
            event.type_ == "user/message"
                && event.data["source"]["kind"] == "goal"
                && event.data["source"]["round"]
                    .as_u64()
                    .is_some_and(|round| round > 0)
        })
        .count();
    assert_eq!(
        positive_goal_messages, 0,
        "the stale round must be rejected before durable admission"
    );
    let events = agent.session().events();
    let turn_end = events
        .iter()
        .find(|event| event.type_ == "turn/end")
        .expect("pre-step rejection must close the turn");
    assert_eq!(turn_end.data["reason"]["kind"], "blocked");
}
