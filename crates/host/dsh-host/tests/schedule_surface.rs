#[test]
fn schedule_lifecycle_contract_is_host_owned() {
    let host = include_str!("../src/lib.rs");
    // Historical session-log reminders stay decodable, but the session-local
    // runtime and tools are no longer installed.
    assert!(host.contains("dsh_schedule::apply_projection(ctx);"));
    assert!(!host.contains("dsh_schedule::apply(ctx);"));
    assert!(host.contains("schedule_tasks::install(ctx, &data_root)"));
    assert!(host.contains("schedule_tasks::attach("));

    let wiring = include_str!("../src/schedule_tasks.rs");
    assert!(wiring.contains("data_root.join(\"schedule.json\")"));
    assert!(wiring.contains("resolve_control_agent(&session_id)"));
    assert!(wiring.contains("lease.agent.followup(message)"));
    assert!(wiring.contains("\"/__dsh-schedule\""));
    assert!(wiring.contains("trusted_web_request(&request, allow_remote)"));
    for operation in ["catalog", "create", "update", "setActive", "delete", "history", "runNow", "wait"] {
        assert!(wiring.contains(&format!("\"{operation}\"")), "{operation}");
    }
}
