use std::sync::Arc;

use dsh_session::{
    Session, SessionHeader, SessionLogOffset, SessionSeq, session_id, turn_start_data,
};

#[test]
fn numeric_session_positions_are_distinct_transparent_serde_numbers() {
    let seq = SessionSeq::new(3).expect("valid event sequence");
    let offset = SessionLogOffset::new(4).expect("valid log offset");

    assert_eq!(serde_json::to_string(&seq).unwrap(), "3");
    assert_eq!(serde_json::to_string(&offset).unwrap(), "4");
    assert_eq!(serde_json::from_str::<SessionSeq>("3").unwrap(), seq);
    assert_eq!(
        serde_json::from_str::<SessionLogOffset>("4").unwrap(),
        offset
    );
    assert!(SessionSeq::new(u64::MAX).is_err());
    assert!(SessionLogOffset::new(u64::MAX).is_err());
}

#[test]
fn full_snapshot_is_cached_until_append_and_event_reads_use_typed_coordinates() {
    let session = Session::create(session_id("snapshot"), None, None, None).unwrap();
    let first = session
        .append("turn/start", turn_start_data(1), None)
        .unwrap();

    assert_eq!(first.seq, SessionSeq::new(0).unwrap());
    assert_eq!(session.seq(), SessionLogOffset::new(1).unwrap());
    assert_eq!(session.event_at(SessionSeq::new(0).unwrap()), Some(first));

    let snapshot = session.snapshot_events(SessionLogOffset::ZERO, None);
    assert!(Arc::ptr_eq(
        &snapshot,
        &session.snapshot_events(SessionLogOffset::ZERO, None)
    ));

    session
        .append(
            "turn/end",
            serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            None,
        )
        .unwrap();
    let appended = session.snapshot_events(SessionLogOffset::ZERO, None);
    assert!(!Arc::ptr_eq(&snapshot, &appended));
    assert_eq!(snapshot.len(), 1);
    assert_eq!(appended.len(), 2);
}

#[test]
fn seeded_lineage_uses_boolean_header_and_exact_separate_cut() {
    let source = Session::create(session_id("source"), None, None, None).unwrap();
    source
        .append("turn/start", turn_start_data(1), None)
        .unwrap();
    source
        .append(
            "turn/end",
            serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            None,
        )
        .unwrap();

    let child_id = session_id("child");
    let header = SessionHeader {
        version: 0,
        id: child_id.clone(),
        created_at: 1,
        cwd: None,
        parent_session: Some(source.id().clone()),
        is_seeded: true,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let child = Session::create(
        child_id,
        Some(
            source
                .snapshot_events(SessionLogOffset::ZERO, None)
                .as_ref()
                .clone(),
        ),
        Some(&header),
        Some(source.seq()),
    )
    .unwrap();

    assert!(child.header().is_seeded);
    assert!(
        serde_json::to_value(child.header())
            .unwrap()
            .get("seedLength")
            .is_none()
    );
    assert_eq!(
        child.inherited_event_count(),
        SessionLogOffset::new(2).unwrap()
    );
    assert_eq!(
        child
            .own_events()
            .iter()
            .map(|event| event.type_.as_str())
            .collect::<Vec<_>>(),
        vec!["session/end-seed"]
    );
    assert!(!child.is_own_seq(SessionSeq::new(1).unwrap()));
    assert!(child.is_own_seq(SessionSeq::new(2).unwrap()));
}
