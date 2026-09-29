use super::*;
use crate::format_v4::{V4ValidationSummary, V4Validator};
use serde_json::{Value, json};

fn events(rows: Vec<(&str, Value)>) -> Vec<SessionEvent> {
    rows.into_iter()
        .enumerate()
        .map(|(seq, (kind, data))| SessionEvent {
            type_: kind.into(),
            seq: crate::SessionSeq::new(seq as u64).unwrap(),
            time: 100 + seq as i64,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        })
        .collect()
}

fn validate(rows: &[SessionEvent], inherited: Option<u64>) -> V4ValidationSummary {
    let mut header = json!({"version":4,"id":"repair-compaction","createdAt":0,"isSeeded":inherited.is_some(),"delegationDepth":0});
    if inherited.is_some() {
        header["parentSession"] = json!("parent");
    }
    let mut validator = V4Validator::new(header, inherited.unwrap_or(0)).unwrap();
    for event in rows {
        validator
            .push(&serde_json::to_value(event).unwrap())
            .unwrap();
    }
    validator.finish().unwrap()
}

#[test]
fn interrupted_compaction_closes_before_its_turn_and_step() {
    for inside_step in [false, true] {
        let mut rows = vec![("turn/start", json!({"turn":1}))];
        if inside_step {
            rows.push(("step/start", json!({"turn":1,"step":1})));
        }
        rows.push((
            "compaction/start",
            json!({"compactionId":"automatic","turn":1}),
        ));
        let original = events(rows);
        assert!(validate(&original, None).open_compaction);
        let closers = interrupted_turn_closers(&original);
        assert_eq!(closers[0].type_, "compaction/end");
        assert_eq!(closers[0].data["compactionId"], "automatic");
        assert_eq!(closers[0].data["turn"], 1);
        assert!(
            closers[0].data["error"]
                .as_str()
                .unwrap()
                .contains("interrupted")
        );
        let expected = if inside_step {
            vec!["compaction/end", "step/end", "turn/end"]
        } else {
            vec!["compaction/end", "turn/end"]
        };
        assert_eq!(
            closers.iter().map(|e| e.type_.as_str()).collect::<Vec<_>>(),
            expected
        );
        let repaired = [original.clone(), closers].concat();
        let result = validate(&repaired, None);
        assert!(!result.open_compaction);
        assert!(result.open_turn.is_none());
        assert!(result.open_step.is_none());
        assert!(interrupted_turn_closers(&repaired).is_empty());
        assert_eq!(&repaired[..original.len()], original.as_slice());
    }
}

#[test]
fn interrupted_manual_compaction_without_a_turn_preserves_its_command_owner() {
    let original = events(vec![(
        "compaction/start",
        json!({"compactionId":"manual","turn":null,"sourceCommandId":"compact-command"}),
    )]);
    assert!(validate(&original, None).open_compaction);
    let closers = interrupted_turn_closers(&original);
    assert_eq!(closers.len(), 1);
    assert_eq!(closers[0].type_, "compaction/end");
    assert_eq!(closers[0].data["turn"], Value::Null);
    assert_eq!(closers[0].data["sourceCommandId"], "compact-command");
    assert!(!validate(&[original, closers].concat(), None).open_compaction);
}

#[test]
fn expired_inherited_compaction_is_not_closed_in_the_new_session() {
    let original = events(vec![
        (
            "compaction/start",
            json!({"compactionId":"parent","turn":null}),
        ),
        ("session/end-seed", json!({"inherited":true})),
        ("turn/start", json!({"turn":1})),
    ]);
    assert!(!validate(&original, Some(1)).open_compaction);
    let closers = interrupted_turn_closers(&original);
    assert_eq!(closers.len(), 1);
    assert_eq!(closers[0].type_, "turn/end");
    assert!(
        validate(&[original, closers].concat(), Some(1))
            .open_turn
            .is_none()
    );
}

#[test]
fn completed_compaction_is_not_repaired_twice() {
    let original = events(vec![
        (
            "compaction/start",
            json!({"compactionId":"done","turn":null}),
        ),
        (
            "compaction/end",
            json!({"compactionId":"done","turn":null,"error":"cancelled"}),
        ),
    ]);
    assert!(!validate(&original, None).open_compaction);
    assert!(interrupted_turn_closers(&original).is_empty());
}
