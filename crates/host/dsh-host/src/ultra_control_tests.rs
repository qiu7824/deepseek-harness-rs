use dsh_agent::Agent;
use dsh_subagent::ultra::UltraControl;
use std::sync::Arc;

async fn parent() -> (cordis::Context, Arc<dyn Agent>) {
    let ctx = cordis::Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    dsh_llm::LlmRuntime::install(&ctx);
    dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    let sessions = dsh_session::SessionStore::install(&ctx);
    let session = sessions
        .create(
            &ctx,
            Some(dsh_session::session_id(uuid::Uuid::new_v4().to_string())),
            None,
        )
        .await
        .unwrap();
    let agent = dsh_agent_loop::ReactLoopAgent::new(
        &ctx,
        session.id().clone(),
        dsh_agent::AgentOptions {
            execution_mode: dsh_llm::ExecutionMode::Ultra,
            ..Default::default()
        },
        session,
    )
    .unwrap();
    (ctx, agent)
}

#[tokio::test]
async fn ultra_fourth_waits_for_release_and_cancel_closes_admission() {
    let (ctx, parent) = parent().await;
    let control = UltraControl::install(&ctx);
    let first = control.admit(&parent, "one").unwrap();
    let second = control.admit(&parent, "two").unwrap();
    let third = control.admit(&parent, "three").unwrap();
    let signal: dsh_subagent::types::SubagentSignal = Arc::new(|| false);
    let fourth = control.admit_wait(&parent, "four", &signal);
    tokio::pin!(fourth);
    assert!(futures::poll!(&mut fourth).is_pending());
    drop(second);
    let fourth = tokio::time::timeout(std::time::Duration::from_secs(1), fourth)
        .await
        .unwrap()
        .unwrap();
    control.close(&parent).await;
    assert_eq!(
        control.admit(&parent, "five").err().unwrap().code,
        "ULTRA_CLOSED"
    );
    drop((first, third, fourth));
}

#[tokio::test]
async fn ultra_cumulative_limits_survive_controller_recreation() {
    let (ctx, parent) = parent().await;
    let control = UltraControl::install(&ctx);
    for index in 0..12 {
        drop(control.admit(&parent, &format!("child-{index}")).unwrap());
    }
    let restored = UltraControl::default();
    let restored = Arc::new(restored);
    assert_eq!(
        restored.admit(&parent, "child-13").err().unwrap().code,
        "ULTRA_CREATE_LIMIT"
    );
    for _ in 12..36 {
        drop(restored.admit(&parent, "child-0").unwrap());
    }
    assert_eq!(
        restored.admit(&parent, "child-0").err().unwrap().code,
        "ULTRA_RUN_LIMIT"
    );
}

#[tokio::test]
async fn ultra_child_depth_and_elapsed_budget_are_enforced_before_creation() {
    let (ctx, root) = parent().await;
    let (_, child) = parent().await;
    let control = UltraControl::install(&ctx);
    dsh_subagent::ultra::mark_child(root.as_ref(), child.as_ref()).unwrap();
    assert_eq!(
        control.admit(&child, "grandchild").err().unwrap().code,
        "ULTRA_DEPTH_LIMIT"
    );
    root.session()
        .append(
            "execution/ultra-admitted",
            serde_json::json!({"childId":"old","epoch":"initial","startedAt":0}),
            None,
        )
        .unwrap();
    assert_eq!(
        control.admit(&root, "new").err().unwrap().code,
        "ULTRA_TIME_LIMIT"
    );
}
