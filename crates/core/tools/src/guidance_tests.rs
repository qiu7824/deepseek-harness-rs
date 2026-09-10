use super::*;
use dsh_system_prompt::{AssembleContext, PromptSection, SystemPrompt};

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
