use dsh_schedule::{schedule_id, schedule_projection_definition};
use dsh_session::{SessionEvent, SessionHeader, SessionSeq, session_id};
use dsh_session_projection::SessionProjectionRegistry;

fn event(type_: &str, seq: u64, data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq: SessionSeq::new(seq).unwrap(),
        time: seq as i64,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn header(is_seeded: bool) -> SessionHeader {
    SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id("projection-test"),
        created_at: 0,
        cwd: None,
        parent_session: None,
        is_seeded,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}

#[test]
fn schedule_projection_preserves_order_and_applies_terminal_changes() {
    let definition = schedule_projection_definition();
    let mut state = (definition.init)(&header(false));
    let create_after = event(
        "schedule/change",
        0,
        serde_json::json!({
            "version": 1,
            "operation": "create",
            "schedule": {
                "kind": "after",
                "id": schedule_id("after"),
                "prompt": "after",
                "afterSeconds": 60,
                "scheduledAt": "2026-08-31T04:00:00.000Z"
            }
        }),
    );
    let create_every = event(
        "schedule/change",
        1,
        serde_json::json!({
            "version": 1,
            "operation": "create",
            "schedule": {
                "kind": "every",
                "id": schedule_id("every"),
                "prompt": "every",
                "everySeconds": 300,
                "scheduledAt": "2026-08-31T04:00:00.000Z"
            }
        }),
    );
    state = (definition.apply)(&state, &create_after);
    state = (definition.apply)(&state, &create_every);
    let view_value = (definition.view)(&state);
    let view: &serde_json::Value = cordis::downcast(&view_value).unwrap();
    assert_eq!(view[0]["id"], "after");
    assert_eq!(view[1]["id"], "every");

    let delete = event(
        "schedule/change",
        2,
        serde_json::json!({"version": 1, "operation": "delete", "id": "after"}),
    );
    state = (definition.apply)(&state, &delete);
    let unrelated = event("turn/start", 3, serde_json::json!({"turn": 1}));
    let same = (definition.apply)(&state, &unrelated);
    assert!(std::sync::Arc::ptr_eq(&state, &same));
    let view_value = (definition.view)(&state);
    let view: &serde_json::Value = cordis::downcast(&view_value).unwrap();
    assert_eq!(view.as_array().unwrap().len(), 1);
    assert_eq!(view[0]["id"], "every");
}

#[test]
#[should_panic(expected = "schedule projection rejected durable event")]
fn schedule_projection_fails_loud_on_corrupt_schedule_change() {
    let definition = schedule_projection_definition();
    let state = (definition.init)(&header(false));
    let corrupt = event(
        "schedule/change",
        0,
        serde_json::json!({"version": 1, "operation": "delete", "id": "missing"}),
    );
    let _ = (definition.apply)(&state, &corrupt);
}

#[test]
fn schedule_projection_replays_inherited_events_into_the_child_view() {
    let definition = schedule_projection_definition();
    let mut state = (definition.init)(&header(true));
    let inherited = event(
        "schedule/change",
        0,
        serde_json::json!({
            "version": 1,
            "operation": "create",
            "schedule": {
                "kind": "after",
                "id": schedule_id("parent"),
                "prompt": "parent",
                "afterSeconds": 60,
                "scheduledAt": "2026-08-31T04:00:00.000Z"
            }
        }),
    );
    let child = event(
        "schedule/change",
        1,
        serde_json::json!({
            "version": 1,
            "operation": "create",
            "schedule": {
                "kind": "after",
                "id": schedule_id("child"),
                "prompt": "child",
                "afterSeconds": 60,
                "scheduledAt": "2026-08-31T04:00:00.000Z"
            }
        }),
    );
    state = (definition.apply)(&state, &inherited);
    state = (definition.apply)(&state, &child);
    let view_value = (definition.view)(&state);
    let view: &serde_json::Value = cordis::downcast(&view_value).unwrap();
    assert_eq!(view.as_array().unwrap().len(), 2);
    assert_eq!(view[0]["id"], "parent");
    assert_eq!(view[1]["id"], "child");
}

#[test]
fn schedule_projection_invalidates_pre_alpha4_seed_cut_checkpoints() {
    assert_eq!(schedule_projection_definition().state_version, 3);
}

#[tokio::test]
async fn schedule_apply_registers_the_projection_in_production_composition() {
    let ctx = cordis::Context::root();
    let registry = SessionProjectionRegistry::install(&ctx);
    dsh_schedule::apply(&ctx);
    assert!(registry.keys().iter().any(|key| key == "schedule"));
}
