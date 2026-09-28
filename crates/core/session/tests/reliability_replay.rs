use dsh_session::{
    Session, SessionEvent,
    chunk_rows::{decode_storage_record, pack_chunk_runs},
    session_id,
};
use serde_json::Value;

#[test]
fn shared_replay_is_lossless_through_packing_archival_and_cold_surface_restore() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../test-fixtures/reliability/session-replay.json"
    )))
    .unwrap();
    let events: Vec<SessionEvent> = serde_json::from_value(fixture["events"].clone()).unwrap();
    let packed = pack_chunk_runs(&events);
    assert!(packed.len() < 100, "long delta runs remain compact on disk");
    let restored: Vec<_> = packed
        .iter()
        .flat_map(|row| decode_storage_record(&row.to_json()).unwrap())
        .collect();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&events).unwrap()
    );
    let session = Session::create(
        session_id("shared-replay"),
        Some(restored.clone()),
        None,
        None,
    )
    .unwrap();
    let cold = Session::from_restore(
        session.id().clone(),
        restored,
        &session.header(),
        session.inherited_event_count(),
    )
    .unwrap();
    assert_eq!(
        cold.surface().unwrap().nodes,
        session.surface().unwrap().nodes
    );
    let messages = cold.with_surface_reader(|reader, nodes| {
        nodes
            .iter()
            .map(|seq| reader.read(*seq).unwrap().unwrap())
            .filter_map(|e| reader.derive_event_message(&e))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        serde_json::to_value(messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()).unwrap(),
        fixture["expected"]["surfaceMessageIds"]
    );
    assert_eq!(
        serde_json::to_value(&messages[1].content).unwrap()[0]["text"],
        fixture["expected"]["assistant"]
    );
    let last = cold
        .find_event_rev(|e| e.type_ == "turn/end")
        .unwrap()
        .unwrap();
    assert_eq!(last.data["reason"]["kind"], "aborted");
    let acceptance = cold
        .find_event_rev(|e| e.data.get("acceptance").is_some())
        .unwrap()
        .unwrap();
    assert_eq!(
        acceptance.data["acceptance"]["status"],
        fixture["expected"]["acceptance"]
    );
}
