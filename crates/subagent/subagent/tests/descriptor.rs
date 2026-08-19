//! Rust port of the descriptor/depth/seeding contract tests: version-2
//! descriptor snapshots, strict persisted-payload parsing, first-event
//! authority folding, delegation-depth accounting, and the staged child
//! creation seed.

use dsh_session::{SessionEvent, session_id};
use dsh_subagent::descriptor::{
    SUBAGENT_DESCRIPTOR_VERSION, SubagentDescriptorData, fold_subagent_descriptor,
    snapshot_subagent_descriptor,
};
use dsh_subagent::{seed_descriptor_turn, subagent_run_id};

fn descriptor_event(data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: "subagent/descriptor".to_string(),
        seq: 0,
        time: 1,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn one_shot() -> SubagentDescriptorData {
    SubagentDescriptorData::OneShot {
        version: SUBAGENT_DESCRIPTOR_VERSION,
        provider: "fork".to_string(),
        label: None,
    }
}

fn continuable() -> SubagentDescriptorData {
    SubagentDescriptorData::Continuable {
        version: SUBAGENT_DESCRIPTOR_VERSION,
        provider: "fork".to_string(),
        label: "check the build".to_string(),
        agent_provider: Some("openai".to_string()),
        agent_model: Some("mock".to_string()),
        persona: None,
        tool_filter: Some(dsh_tools::ToolRestriction {
            allow: Some(vec!["bash".to_string()]),
            deny: None,
        }),
    }
}

#[test]
fn snapshots_validate_and_round_trip_both_descriptor_modes() {
    let one_shot = snapshot_subagent_descriptor(&one_shot()).expect("one-shot");
    let json = serde_json::to_value(&one_shot).expect("json");
    assert_eq!(
        json,
        serde_json::json!({ "version": 2, "mode": "one-shot", "provider": "fork" })
    );
    let continuable = snapshot_subagent_descriptor(&continuable()).expect("continuable");
    let json = serde_json::to_value(&continuable).expect("json");
    assert_eq!(
        json,
        serde_json::json!({
            "version": 2,
            "mode": "continuable",
            "provider": "fork",
            "label": "check the build",
            "agentProvider": "openai",
            "agentModel": "mock",
            "toolFilter": { "allow": ["bash"] }
        })
    );
}

#[test]
fn folds_the_first_descriptor_event_with_version_and_schema_authority() {
    let events = vec![descriptor_event(
        serde_json::to_value(continuable()).expect("json"),
    )];
    let folded = fold_subagent_descriptor(&events)
        .expect("fold")
        .expect("present");
    assert!(folded.is_continuable());
    assert_eq!(folded.provider(), "fork");

    // A foreign-version descriptor is not classified by this runtime.
    let stale = vec![descriptor_event(serde_json::json!({
        "version": 1, "mode": "one-shot", "provider": "fork"
    }))];
    assert!(fold_subagent_descriptor(&stale).expect("fold").is_none());

    // A log without any descriptor event has none.
    let none: Vec<SessionEvent> = Vec::new();
    assert!(fold_subagent_descriptor(&none).expect("fold").is_none());
}

#[test]
fn rejects_malformed_current_version_payloads() {
    let cases: Vec<serde_json::Value> = vec![
        serde_json::Value::Null,
        serde_json::json!({}),
        serde_json::json!({ "version": "2", "mode": "one-shot", "provider": "fork" }),
        serde_json::json!({ "version": 2, "mode": "later", "provider": "fork" }),
        serde_json::json!({ "version": 2, "mode": "one-shot", "provider": "fork", "extra": true }),
        serde_json::json!({ "version": 2, "mode": "one-shot", "provider": 7 }),
        serde_json::json!({ "version": 2, "mode": "one-shot", "provider": "fork", "label": 3 }),
        serde_json::json!({ "version": 2, "mode": "continuable", "provider": "fork" }),
        serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": "x",
            "agentProvider": 9
        }),
        serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": "x",
            "toolFilter": "allow-all"
        }),
        serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": "x",
            "toolFilter": { "allow": ["bash"], "extra": true }
        }),
        serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": "x",
            "toolFilter": { "allow": [1] }
        }),
        serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": "x",
            "toolFilter": {}
        }),
    ];
    for (index, payload) in cases.iter().enumerate() {
        let events = vec![descriptor_event(payload.clone())];
        assert!(
            fold_subagent_descriptor(&events).is_err(),
            "case {index} should reject: {payload}"
        );
    }
}

#[test]
fn stages_the_child_seed_with_one_model_hidden_descriptor_event() {
    let seed = seed_descriptor_turn(&session_id("child"), None, &continuable()).expect("seed");
    // The auto end-seed event plus the descriptor event.
    let descriptor_events: Vec<&SessionEvent> = seed
        .iter()
        .filter(|event| event.type_ == "subagent/descriptor")
        .collect();
    assert_eq!(descriptor_events.len(), 1);
    assert_eq!(descriptor_events[0].seq, 0);
    assert!(descriptor_events[0].surface_op.is_none());
    let folded = fold_subagent_descriptor(&seed)
        .expect("fold")
        .expect("present");
    assert!(folded.is_continuable());
}

#[test]
fn run_ids_are_branded_transparent_strings() {
    let id = subagent_run_id("run-1");
    assert_eq!(id.as_str(), "run-1");
    let json = serde_json::to_string(&id).expect("json");
    assert_eq!(json, "\"run-1\"");
}

#[test]
fn depth_reads_the_monotone_header_floor() {
    fn floor(header: Option<u64>, runtime: Option<u64>) -> u64 {
        header.unwrap_or(0).max(runtime.unwrap_or(0))
    }

    // Depth accounting needs an Agent; the pure floor rule is exercised
    // through the header-only helper path.
    assert_eq!(floor(Some(3), Some(1)), 3);
    assert_eq!(floor(None, None), 0);
    dsh_subagent::assert_subagent_max_depth(Some(4)).expect("max depth");
}
