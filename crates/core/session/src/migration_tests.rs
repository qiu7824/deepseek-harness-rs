use super::*;

fn header() -> SessionHeader {
    SessionHeader {
        version: 0,
        id: crate::session_id("historical"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}
fn event(seq: u64, kind: &str, data: Value) -> SessionEvent {
    SessionEvent {
        type_: kind.into(),
        seq: SessionSeq::new(seq).unwrap(),
        time: seq as i64 + 10,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}
fn epoch(seq: u64, prompt: Option<&str>) -> SessionEvent {
    let mut data = json!({"header":{"config":{}},"reason":"change"});
    if let Some(prompt) = prompt {
        data["header"]["system"] = json!(prompt);
    }
    event(seq, "request/header", data)
}
fn prompts(events: &[SessionEvent]) -> Vec<String> {
    let mut current = String::new();
    let mut result = vec![];
    for event in events {
        if event.type_ == "system/message" {
            current = event
                .data
                .pointer("/message/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into();
        }
        if event.type_ == "request/header" {
            assert!(event.data.pointer("/header/system").is_none());
            result.push(current.clone());
        }
    }
    result
}

#[test]
fn preserves_each_request_prompt_and_clearing_with_deterministic_identities() {
    let source = vec![
        event(0, "step/start", json!({"turn":1,"step":1})),
        epoch(1, Some("first")),
        epoch(2, Some("first")),
        epoch(3, Some(" second ")),
        epoch(4, None),
        epoch(5, Some("")),
    ];
    let before = source.clone();
    let report = migrate_v0_to_v3(header(), &source).unwrap();
    assert_eq!(source, before);
    assert_eq!(report, migrate_v0_to_v3(header(), &source).unwrap());
    assert_eq!(
        prompts(&report.events),
        ["first", "first", " second ", "", ""]
    );
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| e.type_ == "system/message")
            .count(),
        4
    );
    assert_eq!(report.source_offsets, [0, 3, 4, 6, 8, 9, 10]);
    assert_eq!(report.source_cuts, [0, 2, 4, 5, 7, 9, 10]);
    let folded = crate::fold_surface(&report.events).unwrap();
    assert_eq!(folded.nodes, [7]);
}

#[test]
fn aborted_first_step_still_has_an_empty_head_and_non_model_logs_stay_unchanged() {
    let source = vec![
        event(0, "step/start", json!({"turn":2,"step":7})),
        event(1, "step/end", json!({"turn":2,"step":7})),
    ];
    let report = migrate_v0_to_v3(header(), &source).unwrap();
    assert_eq!(report.events[1].type_, "system/message");
    assert_eq!(report.events[1].data["message"]["content"], json!([]));
    let source = vec![event(0, "session/title", json!({"title":"unchanged"}))];
    assert_eq!(migrate_v0_to_v3(header(), &source).unwrap().events, source);
}

#[test]
fn remaps_only_owned_earlier_coordinates_and_preserves_opaque_captures() {
    let mut source = vec![
        epoch(0, Some("A")),
        event(1, "command/run", json!({})),
        epoch(2, Some("B")),
        event(
            3,
            "command/done",
            json!({"sourceEventSeq":1,"throughSeq":1,"sessionFormatVersion":0,"result":{"sourceEventSeq":1}}),
        ),
        event(
            4,
            "compaction/prune",
            json!({"shadowedRange":{"start":0,"end":2},"shadowedSeqs":[0,2]}),
        ),
        event(5, "session/title", json!({"messageSeqs":[1]})),
    ];
    let report = migrate_v0_to_v3(header(), &source).unwrap();
    assert_eq!(
        report.events[5].data,
        json!({"sourceEventSeq":2,"throughSeq":1,"sessionFormatVersion":0,"result":{"sourceEventSeq":1}})
    );
    assert_eq!(
        report.events[6].data,
        json!({"shadowedRange":{"start":1,"end":4},"shadowedSeqs":[1,4]})
    );
    assert_eq!(report.events[7].data["messageSeqs"], json!([2]));
    source[3].data["sourceEventSeq"] = json!(3);
    assert!(
        migrate_v0_to_v3(header(), &source)
            .unwrap_err()
            .contains("earlier")
    );
}

#[test]
fn refuses_malformed_prompts_gaps_already_migrated_nodes_and_identity_collisions() {
    let mut bad = epoch(0, None);
    bad.data["header"]["system"] = json!(true);
    assert!(
        migrate_v0_to_v3(header(), &[bad])
            .unwrap_err()
            .contains("string")
    );
    assert!(
        migrate_v0_to_v3(header(), &[epoch(1, None)])
            .unwrap_err()
            .contains("non-contiguous")
    );
    assert!(migrate_v0_to_v3(header(), &[event(0, "system/message", json!({}))]).is_err());
    let initial = epoch(0, Some("x"));
    let report = migrate_v0_to_v3(header(), &[initial.clone()]).unwrap();
    let collision = event(
        1,
        "user/message",
        json!({"id":report.events[0].data["message"]["id"]}),
    );
    assert!(
        migrate_v0_to_v3(header(), &[initial, collision])
            .unwrap_err()
            .contains("identity collision")
    );
    let mut modern = header();
    modern.version = 3;
    assert!(migrate_v0_to_v3(modern, &[]).is_err());
}

#[test]
fn migration_refuses_unknown_required_events_before_publication_and_preserves_opaque_optional_data()
{
    let mut future = event(
        1,
        "future/required",
        json!({"sourceEventSeq":0,"capturedFormatVersion":0}),
    );
    assert!(
        migrate_v0_to_v3(header(), &[epoch(0, Some("A")), future.clone()])
            .unwrap_err()
            .contains("unsupported required")
    );
    future.ignorable = Some(true);
    let report = migrate_v0_to_v3(header(), &[epoch(0, Some("A")), future.clone()]).unwrap();
    assert_eq!(report.events.last().unwrap().data, future.data);
}
