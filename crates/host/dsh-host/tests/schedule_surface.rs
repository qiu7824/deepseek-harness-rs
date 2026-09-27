#[test]
fn reminders_and_scheduled_tasks_have_distinct_durable_host_domains() {
    let host = include_str!("../src/lib.rs");
    assert!(!host.contains("dsh_schedule::apply(ctx);"));
    assert!(host.contains("dsh_schedule::schedule_projection_definition()"));
    assert!(host.contains("dsh_schedule::host_service::ScheduleService::install("));
    assert!(host.contains("schedule_tasks::install(ctx, &data_root)"));
    assert!(host.contains("schedule_tasks::attach("));
    let wiring = include_str!("../src/schedule_tasks.rs");
    assert!(wiring.contains("data_root.join(\"schedule.json\")"));
    assert!(wiring.contains("api.schedule_session_controller()"));
    assert!(wiring.contains("service.shutdown().await"));
    assert!(wiring.contains("join.await"));
    assert!(wiring.contains("\"/__dsh-schedule\""));
    assert!(wiring.contains("trusted_web_request(&request, allow_remote)"));
    for operation in [
        "catalog",
        "create",
        "update",
        "setActive",
        "delete",
        "history",
        "runNow",
        "wait",
    ] {
        assert!(wiring.contains(&format!("\"{operation}\"")), "{operation}");
    }
}
