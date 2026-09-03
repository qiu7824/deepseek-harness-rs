use cordis::Context;
use dsh_agent::{AgentFactory, AgentRegistry, CreateAgentOptions};
use dsh_agent_loop::AgentLoop;
use dsh_llm::LlmRuntime;
use dsh_session::{
    CreateSessionMeta, SessionEvent, SessionLogOffset, SessionSeq, SessionStore, session_id,
};
use dsh_tools::{Config as ToolsConfig, ToolRuntime};

#[tokio::test]
async fn seeded_agent_creation_preserves_explicit_inherited_event_count() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("system prompt");
    LlmRuntime::install(&ctx);
    ToolRuntime::install(&ctx, ToolsConfig::default()).expect("tools");
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let agent_loop =
        AgentLoop::install(&ctx, dsh_agent_loop::Config::default()).expect("agent loop");

    let seed = vec![
        SessionEvent {
            type_: "turn/start".into(),
            seq: SessionSeq::new(0).unwrap(),
            time: 1,
            data: serde_json::json!({"turn": 1}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        },
        SessionEvent {
            type_: "turn/end".into(),
            seq: SessionSeq::new(1).unwrap(),
            time: 2,
            data: serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        },
    ];
    let inherited = SessionLogOffset::new(seed.len() as u64).unwrap();
    let handle = agent_loop
        .create_agent(
            &ctx,
            CreateAgentOptions {
                session_id: Some(session_id("seeded-agent-create")),
                meta: Some(CreateSessionMeta {
                    parent_session: Some(session_id("seed-parent")),
                    is_seeded: Some(true),
                    ..Default::default()
                }),
                seed: Some(seed),
                inherited_event_count: Some(inherited),
                ..Default::default()
            },
        )
        .await
        .expect("seeded agent creation");

    assert_eq!(handle.agent.session().inherited_event_count(), inherited);
    assert_eq!(handle.agent.session().own_events().len(), 1);
    handle.dispose.await;
    ctx.fiber.dispose().await;
}
