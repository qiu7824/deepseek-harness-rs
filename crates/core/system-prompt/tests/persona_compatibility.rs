use cordis::Context;
use dsh_system_prompt::*;
use serde_json::json;

#[test]
fn legacy_and_explicit_fields_have_one_consistent_parser() {
    assert_eq!(
        parse_config(&json!({"persona":"old"})).unwrap().persona,
        "old"
    );
    assert_eq!(
        parse_config(&json!({"persona":"same","personaPrefix":"same"}))
            .unwrap()
            .persona_prefix
            .as_deref(),
        Some("same")
    );
    assert!(
        parse_config(&json!({"persona":"old","personaPrefix":"new"}))
            .unwrap_err()
            .contains("conflict")
    );
    assert_eq!(
        parse_persona_config(&json!({"text":"old"})).unwrap().prefix,
        "old"
    );
    assert!(parse_persona_config(&json!({"text":"old","prefix":"new"})).is_err());
    assert!(parse_persona_config(&json!({"suffix":false})).is_err());
}

#[tokio::test]
async fn scoped_suffix_shadows_deployment_and_complete_is_exclusive() {
    let ctx = Context::root();
    let service = SystemPrompt::install(
        &ctx,
        parse_config(
            &json!({"personaPrefix":"deployment-prefix", "personaSuffix":"deployment-suffix"}),
        )
        .unwrap(),
    )
    .unwrap();
    service.section(
        &ctx,
        PromptSection {
            name: "tool-guidance".into(),
            order: 100.0,
            text: "tools".into(),
            complete: None,
        },
    );
    let global = service
        .assemble(&ctx, &AssembleContext::default())
        .await
        .unwrap();
    assert!(
        render_prompt(&global).unwrap().find("deployment-prefix")
            < render_prompt(&global).unwrap().find("tools")
    );
    assert!(
        render_prompt(&global).unwrap().find("tools")
            < render_prompt(&global).unwrap().find("deployment-suffix")
    );
    let key = dsh_scope::ScopeKey::new();
    let scope = dsh_scope::create_scope(&ctx, key.clone(), &Default::default());
    let _registrations = service
        .install_persona(&scope.ctx, &json!({"prefix":"preset-prefix"}))
        .unwrap();
    let context = AssembleContext {
        scope: Some(key),
        ..Default::default()
    };
    let rendered = render_prompt(&service.assemble(&scope.ctx, &context).await.unwrap()).unwrap();
    assert!(rendered.contains("preset-prefix"));
    assert!(!rendered.contains("deployment-"));
    (scope.dispose)().await;
    let key = dsh_scope::ScopeKey::new();
    let scope = dsh_scope::create_scope(&ctx, key.clone(), &Default::default());
    let _registrations = service.install_persona(&scope.ctx, &json!({"prefix":"complete", "suffix":"ignored", "complete":true, "includeRuntimeContext":false})).unwrap();
    let assembly = service
        .assemble(
            &scope.ctx,
            &AssembleContext {
                scope: Some(key),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(render_prompt(&assembly).unwrap(), "complete");
    assert!(assembly.contexts.is_empty());
    (scope.dispose)().await;
}
