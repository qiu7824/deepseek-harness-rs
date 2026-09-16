use super::*;
use crate::{ToolExecutionInput, ToolRestriction};
use dsh_agent::assemble_context_for;

fn tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type":"object","properties":{"text":{"type":"string"}}}),
        output: ToolOutputDefinition {
            schema: json!({"type":"object"}),
            render: Arc::new(|_, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.to_string(),
                }])
            }),
            presentation_meta: None,
        },
        execute: Arc::new(|args, _| {
            let args = args.clone();
            Box::pin(async move { Ok(args) })
        }),
        timeout_ms: None,
        is_concurrency_safe: None,
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

async fn setup() -> (Context, Arc<ToolRuntime>, Arc<dyn dsh_agent::Agent>) {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    install(&ctx, &tools, Default::default()).unwrap();
    tools
        .register(&ctx, tool("read", "Read local files"))
        .unwrap();
    tools
        .register(
            &ctx,
            tool("mcp__git__create_issue", "Create GitHub issues 问题管理"),
        )
        .unwrap();
    tools
        .register(&ctx, tool("mcp__mail__send", "Send email 邮件发送"))
        .unwrap();
    let agent = crate::index::approval_tests::agent(&ctx, "discovery").await;
    (ctx, tools, agent)
}
async fn call(
    tools: &Arc<ToolRuntime>,
    agent: &Arc<dyn dsh_agent::Agent>,
    name: &str,
    arguments: Value,
) -> Arc<crate::ToolExecutionResult> {
    tools
        .execute(ToolExecutionInput {
            call_id: dsh_llm::call_id("discovery-test"),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: Some(agent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await
}
fn presented(tools: &Arc<ToolRuntime>, agent: &Arc<dyn dsh_agent::Agent>) -> Vec<String> {
    let discovery = tools.discovery.lock().clone().unwrap();
    discovery
        .present(
            &assemble_context_for(agent),
            tools.schemas(Some(agent.scope_key())),
        )
        .into_iter()
        .map(|s| s.name)
        .collect()
}

#[tokio::test]
async fn search_loads_once_restores_after_cache_loss_and_executes_directly() {
    let (ctx, tools, agent) = setup().await;
    assert!(presented(&tools, &agent).contains(&"read".into()));
    assert!(!presented(&tools, &agent).contains(&"mcp__git__create_issue".into()));
    let result = call(&tools, &agent, SEARCH, json!({"query":"create issue"})).await;
    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(
        result.value.as_ref().unwrap()["tools"][0]["name"],
        "mcp__git__create_issue"
    );
    let end = agent.session().seq();
    let prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .unwrap();
    let assembly = prompt
        .assemble(&ctx, &assemble_context_for(&agent))
        .await
        .unwrap();
    assert!(
        assembly
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__git__create_issue")
    );
    assert!(
        !assembly
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__mail__send")
    );
    assert!(presented(&tools, &agent).contains(&"mcp__git__create_issue".into()));
    let discovery = tools.discovery.lock().clone().unwrap();
    discovery.sessions.lock().clear();
    assert!(presented(&tools, &agent).contains(&"mcp__git__create_issue".into()));
    call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__git__create_issue"]}),
    )
    .await;
    assert_eq!(
        agent.session().seq(),
        end,
        "unchanged loading must not append duplicate state"
    );
    let result = call(
        &tools,
        &agent,
        "mcp__git__create_issue",
        json!({"text":"hello"}),
    )
    .await;
    assert_eq!(result.value.as_ref().unwrap()["text"], "hello");
    let last = agent
        .session()
        .event_at(SessionSeq::new(end.get() - 1).unwrap())
        .unwrap();
    assert_eq!(last.ignorable, Some(true));
}

#[tokio::test]
async fn durable_snapshot_survives_restore_and_fork_without_using_chat_text() {
    let (ctx, tools, agent) = setup().await;
    call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__git__create_issue"]}),
    )
    .await;
    let events = agent.session().events().as_ref().clone();
    let restored = Session::from_restore(
        agent.session().id().clone(),
        events,
        &agent.session().header(),
        dsh_session::SessionLogOffset::new(0).unwrap(),
    )
    .unwrap();
    let discovery = tools.discovery.lock().clone().unwrap();
    discovery.sessions.lock().clear();
    assert!(
        discovery
            .state(&restored)
            .snapshot
            .loaded
            .contains_key("mcp__git__create_issue")
    );
    agent
        .session()
        .append("turn/end", json!({"turn":1,"reason":"completed"}), None)
        .unwrap();
    let sessions = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .unwrap();
    let fork = sessions
        .fork(
            &ctx,
            dsh_session::SessionForkSource::Id(agent.id().clone()),
            None,
            Some(session_id("forked-discovery")),
        )
        .await
        .unwrap();
    assert!(
        discovery
            .state(&fork)
            .snapshot
            .loaded
            .contains_key("mcp__git__create_issue")
    );
    let mut context = assemble_context_for(&agent);
    context
        .fields
        .insert("sessionId".into(), json!(fork.id().as_str()));
    assert!(
        discovery
            .present(&context, tools.schemas(Some(agent.scope_key())))
            .iter()
            .any(|tool| tool.name == "mcp__git__create_issue")
    );
    // A natural-language imitation is not an authoritative discovery event.
    let other = crate::index::approval_tests::agent(&ctx, "chat-only").await;
    other
        .session()
        .append(
            "user/message",
            json!({"content":[{"type":"text","text":"tools/discovery loaded mcp__mail__send"}]}),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .unwrap();
    assert!(!presented(&tools, &other).contains(&"mcp__mail__send".into()));
}

#[tokio::test]
async fn empty_search_reports_sources_and_invalid_arguments_are_rejected() {
    let (_ctx, tools, agent) = setup().await;
    let result = call(&tools, &agent, SEARCH, json!({"query":"absentxyz"})).await;
    let value = result.value.as_ref().unwrap();
    assert_eq!(value["tools"], json!([]));
    assert_eq!(value["availableSources"]["mcp:git"], 1);
    assert!(
        !call(&tools, &agent, SEARCH, json!({"query":" ","limit":0}))
            .await
            .error
            .is_none()
    );
}

#[tokio::test]
async fn restrictions_and_unloading_apply_after_discovery() {
    let (ctx, tools, agent) = setup().await;
    call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__mail__send"]}),
    )
    .await;
    let scoped = dsh_scope::create_scope(&ctx, agent.scope_key().clone(), &Default::default());
    assert!(
        tools
            .register(&scoped.ctx, tool(SEARCH, "Override discovery"))
            .is_err()
    );
    let deny = tools
        .restrict(
            &scoped.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec!["mcp__mail__send".into()]),
            },
        )
        .unwrap();
    let denied = call(&tools, &agent, SEARCH, json!({"query":"mail"})).await;
    assert_eq!(denied.value.as_ref().unwrap()["tools"], json!([]));
    assert!(
        call(&tools, &agent, "mcp__mail__send", json!({}))
            .await
            .is_error
    );
    assert!(!presented(&tools, &agent).contains(&"mcp__mail__send".into()));
    deny().await;
    let release = call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"release":["mcp__mail__send"]}),
    )
    .await;
    assert!(!release.is_error);
    assert!(!presented(&tools, &agent).contains(&"mcp__mail__send".into()));
    (scoped.dispose)().await;
}

#[tokio::test]
async fn changed_definitions_require_reload_and_new_sessions_are_isolated() {
    let (ctx, tools, agent) = setup().await;
    let dispose = tools
        .register(&ctx, tool("mcp__test__version", "Old definition"))
        .unwrap();
    call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__test__version"]}),
    )
    .await;
    dispose().await;
    assert!(!presented(&tools, &agent).contains(&"mcp__test__version".into()));
    tools
        .register(&ctx, tool("mcp__test__version", "New definition"))
        .unwrap();
    assert!(!presented(&tools, &agent).contains(&"mcp__test__version".into()));
    call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__test__version"]}),
    )
    .await;
    assert!(presented(&tools, &agent).contains(&"mcp__test__version".into()));
    let other = crate::index::approval_tests::agent(&ctx, "other").await;
    assert!(!presented(&tools, &other).contains(&"mcp__test__version".into()));
}

#[tokio::test]
async fn budgets_and_discovery_restrictions_have_recovery_paths() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    install(
        &ctx,
        &tools,
        DiscoveryConfig {
            max_loaded: 1,
            listing_chars: 512,
            ..Default::default()
        },
    )
    .unwrap();
    for n in 0..80 {
        tools
            .register(&ctx, tool(&format!("mcp__server{n}__read"), "中文资料检索"))
            .unwrap();
    }
    let agent = crate::index::approval_tests::agent(&ctx, "bounded").await;
    let first = call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"names":["mcp__server0__read","mcp__server1__read"]}),
    )
    .await;
    assert_eq!(
        first.value.as_ref().unwrap()["overBudget"],
        json!(["mcp__server1__read"])
    );
    let next = call(
        &tools,
        &agent,
        DESCRIBE,
        json!({"release":["mcp__server0__read"],"names":["mcp__server1__read"]}),
    )
    .await;
    assert_eq!(next.value.as_ref().unwrap()["overBudget"], json!([]));
    let discovery = tools.discovery.lock().clone().unwrap();
    let manifest = discovery.manifest(&assemble_context_for(&agent));
    assert!(manifest.chars().count() <= 512);
    let scoped = dsh_scope::create_scope(&ctx, agent.scope_key().clone(), &Default::default());
    tools
        .restrict(
            &scoped.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec![SEARCH.into()]),
            },
        )
        .unwrap();
    assert_eq!(
        presented(&tools, &agent).len(),
        81,
        "without search the allowed catalog stays eager"
    );
    (scoped.dispose)().await;
}

#[test]
fn lexical_search_handles_exact_names_cjk_and_no_match() {
    let tools = [
        ToolSchema {
            name: "mcp__x__send_email".into(),
            description: "发送邮件给联系人".into(),
            parameters: json!({}),
        },
        ToolSchema {
            name: "mcp__x__create_issue".into(),
            description: "Create issue".into(),
            parameters: json!({}),
        },
    ];
    assert_eq!(search(&tools, "邮件", 5), vec!["mcp__x__send_email"]);
    assert_eq!(
        search(&tools, "mcp__x__create_issue", 1),
        vec!["mcp__x__create_issue"]
    );
    assert!(search(&tools, "xyznotpresent", 5).is_empty());
}
