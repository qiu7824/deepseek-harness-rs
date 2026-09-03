use dsh_session::{SessionHeader, SessionLogOffset, session_id};
use dsh_session_persistence_jsonl::{HeaderLine, from_header_line, to_header_line};

fn physical(seed_length: Option<u64>) -> HeaderLine {
    HeaderLine {
        type_: "session".to_string(),
        version: 0,
        id: session_id("compat"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        seed_length,
        origin: None,
        delegation_depth: 0,
        agent_preset: None,
    }
}

#[test]
fn v0_seed_length_absent_zero_and_nonzero_map_to_logical_lineage() {
    for (physical, seeded, count) in [
        (physical(None), false, SessionLogOffset::ZERO),
        (physical(Some(0)), true, SessionLogOffset::ZERO),
        (physical(Some(3)), true, SessionLogOffset::new(3).unwrap()),
    ] {
        let decoded = from_header_line(&physical).unwrap();
        assert_eq!(decoded.meta.is_seeded, seeded);
        assert_eq!(decoded.inherited_event_count, count);
    }
}

#[test]
fn logical_header_encodes_seed_length_only_for_seeded_lineage() {
    let header = SessionHeader {
        version: 0,
        id: session_id("compat"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    assert_eq!(to_header_line(&header, None).unwrap().seed_length, None);

    let seeded = SessionHeader {
        is_seeded: true,
        ..header
    };
    assert_eq!(
        to_header_line(&seeded, Some(SessionLogOffset::ZERO))
            .unwrap()
            .seed_length,
        Some(0)
    );
    assert_eq!(
        to_header_line(&seeded, Some(SessionLogOffset::new(3).unwrap()))
            .unwrap()
            .seed_length,
        Some(3)
    );
}
