use super::*;
use dsh_system_prompt::{AssembleContext, PromptSection, SystemPrompt};

#[tokio::test]
async fn delegation_fence_covers_local_late_and_direct_calls_without_masking_parent() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let actor = super::approval_tests::agent(&ctx, "delegation-fence").await;
    let child = dsh_scope::create_scope(&ctx, actor.scope_key().clone(), &Default::default());
    let runs = Arc::new(AtomicUsize::new(0));
    let definition = |name: &str| {
        let runs = runs.clone();
        ToolDefinition {
            name: name.into(),
            description: name.into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            output: ToolOutputDefinition {
                schema: serde_json::json!({"type":"boolean"}),
                render: Arc::new(|_, _| Ok(vec![])),
                presentation_meta: None,
            },
            execute: Arc::new(move |_, _| {
                let runs = runs.clone();
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!(true))
                })
            }),
            timeout_ms: None,
            is_concurrency_safe: None,
            finalize_content: None,
            present_call: None,
            present_result: None,
        }
    };
    for name in ["read", "write"] {
        tools.register(&ctx, definition(name)).unwrap();
    }
    tools
        .register(&child.ctx, definition("schedule_create"))
        .unwrap();
    let input = |name: &str| ToolExecutionInput {
        call_id: dsh_llm::call_id(format!("call-{name}")),
        root_call_id: None,
        name: name.into(),
        arguments: serde_json::json!({}),
        agent: Some(actor.clone()),
        parent: None,
        signal: Arc::new(|| false),
    };
    assert!(!tools.execute(input("schedule_create")).await.is_error);
    tools
        .restrict_all(
            &child.ctx,
            ToolRestriction {
                allow: Some(vec!["read".into()]),
                deny: Some(vec!["optional_delegate".into()]),
            },
        )
        .unwrap();
    tools
        .register(&child.ctx, definition("late_local"))
        .unwrap();
    assert_eq!(
        tools
            .schemas(Some(actor.scope_key()))
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read"]
    );
    for name in ["schedule_create", "late_local", "write"] {
        assert!(tools.execute(input(name)).await.is_error);
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "hidden calls cannot reach tool bodies"
    );
    assert!(
        tools.get("write", None).is_some(),
        "parent tools remain available"
    );
    let other_key = ScopeKey::new();
    let other = dsh_scope::create_scope(&ctx, other_key.clone(), &Default::default());
    tools
        .restrict_all(
            &other.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec!["optional_delegate".into()]),
            },
        )
        .unwrap();
    tools
        .register(&other.ctx, definition("optional_delegate"))
        .unwrap();
    assert!(
        tools.get("optional_delegate", Some(&other_key)).is_none(),
        "late optional delegation remains denied"
    );
    assert!(tools.get("read", Some(&other_key)).is_some());
    tools
        .register(&other.ctx, definition("parent_private"))
        .unwrap();
    assert!(tools.get("parent_private", None).is_none());
    let nested_key = ScopeKey::new();
    let nested = dsh_scope::create_scope(&ctx, nested_key.clone(), &Default::default());
    tools
        .inherit_visible(&nested.ctx, &tools, &other_key)
        .unwrap();
    assert!(
        tools.get("parent_private", Some(&nested_key)).is_some(),
        "delegation retains scoped parent tools without inheriting conversation state"
    );
    tools
        .register(&nested.ctx, definition("optional_delegate"))
        .unwrap();
    assert!(
        tools.get("optional_delegate", Some(&nested_key)).is_none(),
        "parent capability fences also constrain child-local tools"
    );
    (nested.dispose)().await;
    (other.dispose)().await;
    (child.dispose)().await;
}

#[tokio::test]
async fn child_guidance_and_execution_share_visibility_without_masking_parent_tools() {
    let ctx = Context::root();
    let prompt = SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    for name in ["read", "write"] {
        tools
            .register(
                &ctx,
                ToolDefinition {
                    name: name.into(),
                    description: name.into(),
                    parameters: serde_json::json!({"type":"object","properties":{}}),
                    output: ToolOutputDefinition {
                        schema: serde_json::json!({"type":"boolean"}),
                        render: Arc::new(|_, _| Ok(vec![])),
                        presentation_meta: None,
                    },
                    execute: Arc::new(|_, _| Box::pin(async { Ok(serde_json::json!(true)) })),
                    timeout_ms: None,
                    is_concurrency_safe: None,
                    finalize_content: None,
                    present_call: None,
                    present_result: None,
                },
            )
            .unwrap();
        prompt.section(
            &ctx,
            PromptSection {
                name: format!("tool:{name}"),
                order: 100.0,
                text: scoped_tool_guidance(&ctx, &[name], format!("Use {name}.")),
                complete: None,
            },
        );
    }
    let scope = ScopeKey::new();
    let child_scope = dsh_scope::create_scope(&ctx, scope.clone(), &Default::default());
    let child = child_scope.ctx.clone();
    tools
        .restrict(
            &child,
            ToolRestriction {
                allow: Some(vec!["read".into()]),
                deny: None,
            },
        )
        .unwrap();
    let limited = prompt
        .assemble(
            &child,
            &AssembleContext {
                scope: Some(scope.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        limited
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read"]
    );
    assert_eq!(
        limited
            .sections
            .iter()
            .find(|section| section.name == "tool:read")
            .unwrap()
            .text,
        "Use read."
    );
    assert!(
        limited
            .sections
            .iter()
            .find(|section| section.name == "tool:write")
            .unwrap()
            .text
            .is_empty()
    );
    let _code = tools
        .present_as(&child, ToolPresentationMode::Code)
        .unwrap();
    let code = prompt
        .assemble(
            &child,
            &AssembleContext {
                scope: Some(scope),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        code.sections
            .iter()
            .find(|section| section.name == "tool:read")
            .unwrap()
            .text,
        "Use read.",
        "logical tool guidance survives code presentation"
    );
    assert!(
        code.sections
            .iter()
            .find(|section| section.name == "tool:write")
            .unwrap()
            .text
            .is_empty()
    );
    let parent = prompt.assemble(&ctx, &Default::default()).await.unwrap();
    assert_eq!(parent.tools.len(), 2);
    assert_eq!(
        parent
            .sections
            .iter()
            .find(|section| section.name == "tool:write")
            .unwrap()
            .text,
        "Use write."
    );
    (child_scope.dispose)().await;
}
