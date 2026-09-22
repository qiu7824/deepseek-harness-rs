use super::idle_retirement_tests::control_admission_fixture;
use super::*;
use crate::api::goals::{GoalCreateRequest, GoalEditRequest, GoalVerbRequest};
use dsh_goal::{
    GoalCompletionCommitGuard, GoalCompletionError, GoalCompletionGuard, GoalCompletionPermit,
    GoalRequirementsIdentity,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct AcceptanceGuard {
    bound: GoalRequirementsIdentity,
    calls: Arc<AtomicUsize>,
}
struct Permit;
struct Commit;
impl GoalCompletionCommitGuard for Commit {}
impl GoalCompletionPermit for Permit {
    fn check<'a>(
        &'a self,
        _: &Arc<dyn Agent>,
        _: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionCommitGuard + 'a>, GoalCompletionError> {
        Ok(Box::new(Commit))
    }
}
#[async_trait::async_trait]
impl GoalCompletionGuard for AcceptanceGuard {
    async fn prepare(
        &self,
        _: &Arc<dyn Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> Result<Box<dyn GoalCompletionPermit>, GoalCompletionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if identity != &self.bound {
            return Err(GoalCompletionError::Blocked(
                "acceptance belongs to the previous objective".into(),
            ));
        }
        Ok(Box::new(Permit))
    }
}
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
async fn goal_complete_rpc_uses_same_requirements_guard_as_model_tools() {
    let (service, agent, detach) = control_admission_fixture("goal-rpc-guard").await;
    let goals = dsh_goal::GoalService::install(agent.ctx(), dsh_goal::Config::default());
    assert!(
        service
            .goal_create(create(&agent, "A"))
            .await
            .result
            .is_ok()
    );
    let first = goals.get(&agent).unwrap().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let guard: Arc<dyn GoalCompletionGuard> = Arc::new(AcceptanceGuard {
        bound: goals.requirements_identity(&agent).unwrap().unwrap(),
        calls: calls.clone(),
    });
    agent.ctx().reflect.provide(
        agent.ctx(),
        dsh_goal::GOAL_COMPLETION_GUARD_SERVICE,
        Some(cordis::arc(guard)),
        None,
    );
    let edited = service
        .goal_edit(req(GoalEditRequest {
            session_id: agent.id().clone(),
            goal_ref: ApiProxyService::wire_goal_ref(&first),
            objective: Some("B".into()),
            max_goal_rounds: None,
        }))
        .await;
    assert!(edited.result.is_ok());
    let current = goals.get(&agent).unwrap().unwrap();
    assert_eq!(current.objective, "B");
    let completed = service
        .goal_verb(verb(&agent, &current), GoalVerb::Complete)
        .await;
    assert!(!completed.result.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        goals.get(&agent).unwrap().unwrap().phase,
        dsh_goal::GoalPhase::Active
    );
    detach().await;
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
    ] {
        assert!(goal_requirements_command(line), "{line}");
    }
    for line in [
        "/goal",
        "/goal pause",
        "/goal RESUME",
        "/goal clear",
        "/goal edit",
        "/goals New",
        "not a command",
    ] {
        assert!(!goal_requirements_command(line), "{line}");
    }
}
