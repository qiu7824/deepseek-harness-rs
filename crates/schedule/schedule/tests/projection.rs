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
                "everySeconds": 60,
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
fn schedule_projection_invalidates_checkpoints_that_could_drop_titles() {
    assert_eq!(schedule_projection_definition().state_version, 4);
}

#[tokio::test]
async fn schedule_apply_registers_the_projection_in_production_composition() {
    let ctx = cordis::Context::root();
    let registry = SessionProjectionRegistry::install(&ctx);
    dsh_schedule::apply(&ctx);
    assert!(registry.keys().iter().any(|key| key == "schedule"));
}

fn historical_record(kind: &str, title: Option<serde_json::Value>) -> serde_json::Value {
    let mut record = serde_json::json!({
        "id": kind,
        "kind": kind,
        "prompt": "Check progress\nReview remaining work",
        "scheduledAt": "2026-09-27T09:00:00.000Z"
    });
    if kind == "after" {
        record["afterSeconds"] = serde_json::json!(60);
    }
    if kind == "every" {
        record["everySeconds"] = serde_json::json!(60);
    }
    if let Some(title) = title {
        record["title"] = title;
    }
    record
}

fn historical_create(record: serde_json::Value, seq: u64) -> SessionEvent {
    event(
        "schedule/change",
        seq,
        serde_json::json!({"version": 1, "operation": "create", "schedule": record}),
    )
}

#[test]
fn historical_named_and_unnamed_records_round_trip_without_content_loss() {
    for title in [None, Some(serde_json::json!("Release review 😀"))] {
        let definition = schedule_projection_definition();
        let mut state = (definition.init)(&header(false));
        let records: Vec<_> = ["after", "at", "every"]
            .into_iter()
            .map(|kind| historical_record(kind, title.clone()))
            .collect();
        let events: Vec<_> = records
            .iter()
            .enumerate()
            .map(|(index, record)| historical_create(record.clone(), index as u64))
            .collect();
        let before = serde_json::to_vec(&events).unwrap();
        for event in &events {
            let decoded = dsh_schedule::decode_schedule_change(&event.data).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), event.data);
            state = (definition.apply)(&state, event);
        }
        let view = (definition.schema)(&(definition.view)(&state)).unwrap();
        assert_eq!(view, serde_json::json!(records));
        for record in view.as_array().unwrap() {
            assert_eq!(record.get("title"), title.as_ref());
        }
        let folded = dsh_schedule::fold_schedule_events(&events, 0).unwrap();
        assert_eq!(serde_json::to_value(folded.active).unwrap(), view);
        assert_eq!(serde_json::to_vec(&events).unwrap(), before);
    }
}

#[test]
fn historical_present_titles_use_utf16_limits_and_javascript_whitespace() {
    for kind in ["after", "at", "every"] {
        for title in [
            serde_json::Value::Null,
            serde_json::json!(7),
            serde_json::json!(""),
            serde_json::json!(" padded"),
            serde_json::json!("padded\u{feff}"),
            serde_json::json!("😀".repeat(61)),
        ] {
            let source = historical_create(historical_record(kind, Some(title)), 0);
            assert!(
                dsh_schedule::decode_schedule_change(&source.data).is_err(),
                "{kind}: {}",
                source.data
            );
        }
        for title in ["😀".repeat(60), "\u{85}".to_owned()] {
            let source =
                historical_create(historical_record(kind, Some(serde_json::json!(title))), 0);
            let decoded = dsh_schedule::decode_schedule_change(&source.data).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), source.data);
        }
        let mut record = historical_record(kind, None);
        record["extra"] = serde_json::json!(true);
        assert!(dsh_schedule::decode_schedule_change(&historical_create(record, 0).data).is_err());
    }
}

#[test]
fn historical_recurring_dispatch_preserves_optional_title() {
    for title in [None, Some(serde_json::json!("Retained task name"))] {
        let mut expected = historical_record("every", title);
        let events = vec![
            historical_create(expected.clone(), 0),
            event(
                "schedule/change",
                1,
                serde_json::json!({
                    "version": 1, "operation": "dispatch", "id": "every", "acceptedAt": "2026-09-27T09:04:15.000Z"
                }),
            ),
        ];
        let folded = dsh_schedule::fold_schedule_events(&events, 0).unwrap();
        expected["scheduledAt"] = serde_json::json!("2026-09-27T09:05:00.000Z");
        assert_eq!(
            serde_json::to_value(folded.active).unwrap(),
            serde_json::json!([expected])
        );
    }
}

#[test]
fn historical_projection_refuses_invalid_titles_and_extra_checkpoint_fields() {
    let definition = schedule_projection_definition();
    let original = historical_record("at", Some(serde_json::json!("Valid name")));
    for (key, value) in [
        ("title", serde_json::Value::Null),
        ("title", serde_json::json!(" untrimmed")),
        ("extra", serde_json::json!("unexpected")),
    ] {
        let mut invalid = original.clone();
        invalid[key] = value;
        assert!((definition.schema)(&cordis::arc(serde_json::json!([invalid.clone()]))).is_err());
        let state = cordis::arc(serde_json::json!({"active": [invalid], "seenIds": ["at"]}));
        let read =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (definition.view)(&state)));
        assert!(
            read.is_err(),
            "checkpoint must use the strict record decoder"
        );
    }
}

#[tokio::test]
async fn historical_version_three_checkpoint_replays_titles_from_original_events() {
    use dsh_session_projection::{ProjectionCheckpoint, ProjectionCheckpointRow};
    let ctx = cordis::Context::root();
    let registry = SessionProjectionRegistry::install(&ctx);
    registry
        .register(&ctx, schedule_projection_definition())
        .unwrap();
    let named = historical_record("at", Some(serde_json::json!("Restored task name")));
    let events = vec![historical_create(named.clone(), 0)];
    let mut old_record = named.clone();
    old_record.as_object_mut().unwrap().remove("title");
    let mut checkpoint = ProjectionCheckpoint::new();
    checkpoint.insert(
        "schedule".to_owned(),
        ProjectionCheckpointRow {
            ver: 3,
            seq: 0,
            val: serde_json::json!({"active": [old_record], "seenIds": ["at"]}),
        },
    );
    let (snapshot, refreshed) = registry
        .restore(&header(false), &checkpoint, &events, 0)
        .unwrap();
    assert_eq!(
        snapshot.values["schedule"],
        serde_json::json!([named.clone()])
    );
    assert_eq!(refreshed["schedule"].ver, 4);
    assert_eq!(refreshed["schedule"].val["active"][0], named);
    assert!(
        registry
            .restore(&header(false), &checkpoint, &[], 1)
            .is_err()
    );
    ctx.fiber.dispose().await;
}
