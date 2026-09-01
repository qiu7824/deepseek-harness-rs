#[test]
fn schedule_lifecycle_contract_is_composed_and_global() {
    let host = include_str!("../src/lib.rs");
    assert!(host.contains("dsh_schedule::apply(ctx);"));

    let schedule = include_str!("../../../schedule/schedule/src/lib.rs");
    assert!(schedule.contains("agent/session-start"));
    assert!(schedule.contains("EventOptions::default().global(true)"));
    assert!(schedule.contains("for root in registry.roots()"));
    assert!(schedule.contains("attach_root(root)"));
    assert!(schedule.contains("registry.is_owned_by(agent.id(), &agent)"));
}
