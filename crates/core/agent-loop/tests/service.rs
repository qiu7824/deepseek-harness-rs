//! AgentLoop service tests: Rust port of the core
//! `packages/core/agent-loop/tests/config-session-id.spec.ts` +
//! `agent-initiator.spec.ts` behaviors (configured-agent startup, factory
//! publication, identity conflicts, and owned teardown).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use dsh_agent::{AgentFactory, AgentRegistry};
use dsh_agent_loop::{AgentLoop, Config, ConfiguredAgent};
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk};
use dsh_session::{SessionStore, session_id};

fn script() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::TextDelta {
            index: 0,
            text: "hi".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: dsh_llm::ContentBlock::Text {
                text: "hi".to_string(),
            },
        },
        StreamChunk::Finish {
            reason: dsh_llm::FinishReason::Stop,
            replay_state: None,
        },
    ]
}

struct ScriptedAdapter;

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(script()))
    }
}

fn setup() -> (Context, Arc<SessionStore>, Arc<AgentRegistry>) {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let llm = LlmRuntime::install(&ctx);
    llm.register_adapter(&ctx, vec!["test".to_string()], Arc::new(ScriptedAdapter))
        .expect("adapter");
    let _ = dsh_tools::ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let store = SessionStore::install(&ctx);
    let agents = AgentRegistry::install(&ctx);
    (ctx, store, agents)
}

fn configured(id: &str, session: &str) -> ConfiguredAgent {
    ConfiguredAgent {
        id: id.to_string(),
        session_id: Some(session_id(session)),
        cwd: None,
        resume_session_id: None,
        options: dsh_agent::AgentOptions {
            provider: Some("test".to_string()),
            model: Some("model".to_string()),
            max_tokens: None,
            subagent_depth: None,
        },
    }
}

#[tokio::test]
async fn install_creates_configured_agents_and_publishes_the_factory() {
    let (ctx, store, agents) = setup();
    let loop_service = AgentLoop::install(
        &ctx,
        Config {
            max_parallel_tool_calls: None,
            agents: vec![configured("main", "configured-session")],
        },
    )
    .expect("install");

    // The configured agent is live in both registries.
    assert!(agents.get(&session_id("configured-session")).is_some());
    assert!(store.get(&session_id("configured-session")).is_some());

    // The factory is published: createAgent through it works.
    let handle = loop_service
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("created-session")),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    max_tokens: None,
                    subagent_depth: None,
                }),
                ..Default::default()
            },
        )
        .await
        .expect("createAgent");
    assert_eq!(handle.agent.id().as_str(), "created-session");
    assert!(store.get(&session_id("created-session")).is_some());

    // The owned handle does not resolve until teardown is complete.
    handle.dispose.await;
    assert!(agents.get(&session_id("created-session")).is_none());
    assert!(store.get(&session_id("created-session")).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_dispose_waiter_does_not_cancel_agent_teardown() {
    let (ctx, store, agents) = setup();
    let loop_service = AgentLoop::install(&ctx, Config::default()).expect("install");
    let handle = loop_service
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("cancelled-dispose-waiter")),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("createAgent");
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let entered_for_effect = entered.clone();
    let release_for_effect = release.clone();
    let _gate = handle.agent.ctx().effect(
        "test.gated-dispose",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                let entered = entered_for_effect.clone();
                let release = release_for_effect.clone();
                Box::pin(async move {
                    entered.store(true, Ordering::SeqCst);
                    while !release.load(Ordering::SeqCst) {
                        tokio::task::yield_now().await;
                    }
                })
            }))
        }),
    );
    tokio::task::yield_now().await;

    let waiter = tokio::spawn(handle.dispose);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("agent teardown must enter the gated disposer");
    waiter.abort();
    release.store(true, Ordering::SeqCst);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while agents
            .get(&session_id("cancelled-dispose-waiter"))
            .is_some()
            || store.get(&session_id("cancelled-dispose-waiter")).is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("agent teardown must survive cancellation of one waiter");
}

#[tokio::test]
async fn agents_created_from_an_agent_scope_retain_their_exact_parent() {
    let (ctx, _store, agents) = setup();
    let loop_service = AgentLoop::install(&ctx, Config::default()).expect("install");
    let parent = loop_service
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("owner-parent")),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("parent");
    let child = loop_service
        .create_agent(
            parent.agent.ctx(),
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("owned-child")),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("child");

    assert!(
        agents.is_owned_by(child.agent.id(), &parent.agent),
        "the child must retain the exact owner from parent.agent.ctx()"
    );
    assert!(
        !agents
            .roots()
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, &child.agent)),
        "an owned child must not be classified as a root"
    );

    child.dispose.await;
    parent.dispose.await;
}

#[tokio::test]
async fn configured_identity_conflicts_reject_at_install() {
    let (ctx, _store, _agents) = setup();
    let error = AgentLoop::install(
        &ctx,
        Config {
            max_parallel_tool_calls: None,
            agents: vec![configured("a", "same"), configured("b", "same")],
        },
    )
    .err()
    .expect("duplicate identity must reject");
    assert!(
        error.contains("duplicate exact session identity"),
        "got {error}"
    );
}

#[tokio::test]
async fn session_and_resume_identities_are_mutually_exclusive() {
    let (ctx, _store, _agents) = setup();
    let mut entry = configured("both", "s1");
    entry.resume_session_id = Some(session_id("s2"));
    let error = AgentLoop::install(
        &ctx,
        Config {
            max_parallel_tool_calls: None,
            agents: vec![entry],
        },
    )
    .err()
    .expect("mutually exclusive identities must reject");
    assert!(error.contains("mutually exclusive"), "got {error}");
}

#[tokio::test]
async fn resume_without_persistence_rejects() {
    let (ctx, _store, _agents) = setup();
    let loop_service = AgentLoop::install(&ctx, Config::default()).expect("install");
    let error = loop_service
        .resume(
            &ctx,
            dsh_agent::ResumeAgentOptions {
                resume_session_id: Some(session_id("persisted")),
                ..Default::default()
            },
        )
        .await
        .err()
        .expect("resume without persistence must reject");
    assert!(error.contains("not configured"), "got {error}");
}
