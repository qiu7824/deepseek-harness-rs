use super::idle_retirement_tests::control_admission_fixture;
use super::*;
use std::time::Duration;

#[tokio::test]
async fn try_control_admission_returns_busy_without_resolving_or_waiting() {
    let ctx = Context::root();
    let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
    let id = dsh_session::session_id("unresolved-held-control");
    let held = service.resolver.admission(&id).lock_owned().await;
    // This context deliberately has no factory or persistence. Resolving the
    // identity would fail instead of returning the contention result.
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        service.try_resolve_control_agent(id.as_str()),
    )
    .await
    .expect("try admission must never queue behind the current operation")
    .unwrap();
    assert!(result.is_none());
    drop(held);
}

#[tokio::test]
async fn try_control_admission_holds_exact_owner_and_preserves_waiting_api() {
    let (service, agent, detach) = control_admission_fixture("live-control").await;
    let lease = service
        .try_resolve_control_agent(agent.id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&lease.agent, &agent));
    assert!(
        service
            .try_resolve_control_agent(agent.id().as_str())
            .await
            .unwrap()
            .is_none()
    );
    let mut waiting = Box::pin(service.resolve_control_agent(agent.id().as_str()));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err(),
        "the legacy resolver retains its waiting contract"
    );
    drop(lease);
    let second = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&second.agent, &agent));
    drop(second);
    detach().await;
}

#[tokio::test]
async fn try_control_admission_failure_releases_its_gate() {
    let ctx = Context::root();
    let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
    let id = dsh_session::session_id("missing-control-owner");
    assert!(
        service
            .try_resolve_control_agent(id.as_str())
            .await
            .is_err()
    );
    let gate = service
        .resolver
        .admission(&id)
        .try_lock_owned()
        .expect("failed restoration must not retain admission");
    drop(gate);
    assert!(
        service
            .try_resolve_control_agent(id.as_str())
            .await
            .is_err(),
        "a fresh retry reaches resolution, not false busy"
    );
}

#[tokio::test]
async fn try_control_admission_allows_resolution_wait_and_fences_competitors() {
    let (service, agent, detach) = control_admission_fixture("restoring-control").await;
    let retirement = service.resolver.begin_retirement(agent.id());
    let mut restoring = Box::pin(service.try_resolve_control_agent(agent.id().as_str()));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut restoring)
            .await
            .is_err(),
        "winning admission may await the normal restoration boundary"
    );
    assert!(
        service
            .try_resolve_control_agent(agent.id().as_str())
            .await
            .unwrap()
            .is_none(),
        "a concurrent edit must not join or queue behind restoration"
    );
    drop(retirement);
    let lease = tokio::time::timeout(Duration::from_secs(1), restoring)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&lease.agent, &agent));
    drop(lease);
    detach().await;
}
