use super::idle_retirement_tests::control_admission_fixture;
use super::*;
use crate::api::goals::{GoalCreateRequest, GoalEditRequest, GoalVerbRequest};
fn req<T>(payload: T) -> RpcRequest<T> {
    RpcRequest {
        rpc_id: crate::api::rpc::rpc_id("goal-test"),
        payload,
    }
}
fn create(agent: &Arc<dyn Agent>, objective: &str) -> RpcRequest<GoalCreateRequest> {
    req(GoalCreateRequest {
        session_id: agent.id().clone(),
        objective: objective.into(),
        max_goal_rounds: Some(8),
    })
}
fn verb(agent: &Arc<dyn Agent>, goal: &dsh_goal::GoalView) -> RpcRequest<GoalVerbRequest> {
    req(GoalVerbRequest {
        session_id: agent.id().clone(),
        goal_ref: ApiProxyService::wire_goal_ref(goal),
    })
}

#[tokio::test]
async fn simple_goal_completion_remains_available_but_completed_edit_cannot_inherit_it() {
    let (service, agent, detach) = control_admission_fixture("simple-goal-rpc").await;
    let goals = dsh_goal::GoalService::install(agent.ctx(), dsh_goal::Config::default());
    assert!(
        service
            .goal_create(create(&agent, "A"))
            .await
            .result
            .is_ok()
    );
    let original = goals.get(&agent).unwrap().unwrap();
    assert!(
        service
            .goal_verb(verb(&agent, &original), GoalVerb::Complete)
            .await
            .result
            .is_ok()
    );
    let completed = goals.get(&agent).unwrap().unwrap();
    let seq = agent.session().seq();
    let edited = service
        .goal_edit(req(GoalEditRequest {
            session_id: agent.id().clone(),
            goal_ref: ApiProxyService::wire_goal_ref(&completed),
            objective: Some("B".into()),
            max_goal_rounds: None,
        }))
        .await;
    assert!(!edited.result.is_ok());
    assert_eq!(agent.session().seq(), seq);
    assert_eq!(goals.get(&agent).unwrap().unwrap().objective, "A");
    assert!(
        service
            .goal_create(create(&agent, "B"))
            .await
            .result
            .is_ok()
    );
    let fresh = goals.get(&agent).unwrap().unwrap();
    assert_ne!(fresh.id, original.id);
    assert_eq!(fresh.phase, dsh_goal::GoalPhase::Active);
    detach().await;
}

#[tokio::test]
async fn user_goal_requirements_reject_pending_prompt_admission_but_budget_and_pause_work() {
    let (service, agent, detach) = control_admission_fixture("goal-pending-input").await;
    let goals = dsh_goal::GoalService::install(agent.ctx(), dsh_goal::Config::default());
    assert!(
        service
            .goal_create(create(&agent, "A"))
            .await
            .result
            .is_ok()
    );
    let original = goals.get(&agent).unwrap().unwrap();
    let identity = goals.requirements_identity(&agent).unwrap();
    let lease = service.resolver.admission(agent.id()).lock_owned().await;
    assert!(matches!(
        service.goal_create(create(&agent, "B")).await.result,
        RpcResult::Err {
            error: RpcError::AgentBusy(_),
            ..
        }
    ));
    let request = req(GoalEditRequest {
        session_id: agent.id().clone(),
        goal_ref: ApiProxyService::wire_goal_ref(&original),
        objective: Some("B".into()),
        max_goal_rounds: None,
    });
    assert!(matches!(
        service.goal_edit(request).await.result,
        RpcResult::Err {
            error: RpcError::AgentBusy(_),
            ..
        }
    ));
    let budget = req(GoalEditRequest {
        session_id: agent.id().clone(),
        goal_ref: ApiProxyService::wire_goal_ref(&original),
        objective: None,
        max_goal_rounds: Some(16),
    });
    assert!(service.goal_edit(budget).await.result.is_ok());
    let updated = goals.get(&agent).unwrap().unwrap();
    assert!(
        service
            .goal_verb(verb(&agent, &updated), GoalVerb::Pause)
            .await
            .result
            .is_ok()
    );
    assert_eq!(goals.requirements_identity(&agent).unwrap(), identity);
    drop(lease);
    detach().await;
}

#[test]
fn goal_slash_requirement_commands_share_the_admission_fence() {
    for line in [
        "/goal New objective",
        "/goal edit New objective",
        "/goal EDIT\nAnother objective",
        "/goal clear",
    ] {
        assert!(goal_requirements_command(line), "{line}");
    }
    for line in [
        "/goal",
        "/goal pause",
        "/goal RESUME",
        "/goal edit",
        "/goals New",
        "not a command",
    ] {
        assert!(!goal_requirements_command(line), "{line}");
    }
}
