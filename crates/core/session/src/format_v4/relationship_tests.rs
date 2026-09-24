use super::*;
use serde_json::{Value, json};

fn header(seeded: bool) -> Value {
    let mut h = json!({"version":4,"id":"s","createdAt":0,"isSeeded":seeded,"delegationDepth":0});
    if seeded {
        h["parentSession"] = json!("parent");
    }
    h
}
fn event(kind: &str, data: Value) -> Value {
    json!({"type":kind,"data":data})
}
fn message(kind: &str, id: &str, role: &str, source: Value, content: Value) -> Value {
    let m = json!({"id":id,"role":role,"source":source,"content":content});
    let mut e = event(
        kind,
        if kind == "user/message" {
            m
        } else {
            json!({"turn":1,"step":1,"message":m})
        },
    );
    e["surfaceOp"] = json!("append");
    e
}
fn user(id: &str) -> Value {
    message(
        "user/message",
        id,
        "user",
        json!({"kind":"user"}),
        json!([{"type":"text","text":id}]),
    )
}
fn prefix() -> Vec<Value> {
    vec![
        event("turn/start", json!({"turn":1})),
        event("step/start", json!({"turn":1,"step":1})),
        message(
            "system/message",
            "system",
            "system",
            json!({"kind":"system-prompt"}),
            json!([]),
        ),
        user("u"),
        event(
            "request/header",
            json!({"header":{"config":{"provider":"mock","model":"model"},"tools":[{"name":"read","description":"Read","parameters":{"type":"object"}}]}}),
        ),
    ]
}
fn base() -> Vec<Value> {
    let mut rows = prefix();
    rows.extend([
    message("assistant/message","a","assistant",json!({"kind":"model","provider":"mock","model":"model"}),json!([{"type":"tool-call","id":"c","name":"read","arguments":"{}"}])),
    event("tool/call",json!({"turn":1,"step":1,"callId":"c","name":"read","arguments":"{}"})),
    json!({"type":"tool/result","surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"r","role":"tool","source":{"kind":"tool","callId":"c"},"toolCallId":"c","isError":false,"content":[{"type":"text","text":"ok"}]}}}),
    event("step/end",json!({"turn":1,"step":1})),event("turn/end",json!({"turn":1,"reason":{"kind":"completed"}}))]);
    rows
}
fn numbered(rows: Vec<Value>) -> Vec<Value> {
    rows.into_iter()
        .enumerate()
        .map(|(i, mut row)| {
            row["seq"] = json!(i);
            row["time"] = json!(100 + i);
            row
        })
        .collect()
}
fn validate(rows: Vec<Value>, seeded: bool, cut: u64) -> Result<V4ValidationSummary, String> {
    let mut v = V4Validator::new(header(seeded), cut)?;
    for row in numbered(rows) {
        v.push(&row)?;
    }
    v.finish()
}

#[test]
fn balanced_lifecycle_and_open_tails_are_distinct_from_invalid_closure() {
    let rows = base();
    let closed = validate(rows.clone(), false, 0).unwrap();
    assert_eq!(closed.open_turn, None);
    assert_eq!(closed.pending_tools, 0);
    let open = validate(rows[..7].to_vec(), false, 0).unwrap();
    assert_eq!(open.open_turn, Some(1));
    assert_eq!(open.open_step, Some(1));
    assert_eq!(open.pending_tools, 1);
    let mut bad = rows.clone();
    bad.remove(7);
    assert!(validate(bad, false, 0).unwrap_err().contains("unresolved"));
    let mut bad = rows;
    bad.remove(8);
    assert!(validate(bad, false, 0).is_err());
}

#[test]
fn advertised_tool_identity_start_and_result_must_match_exactly() {
    for (index, pointer, value) in [
        (6, "/data/arguments", json!("different")),
        (6, "/data/name", json!("other")),
        (6, "/data/callId", json!("other")),
        (7, "/data/step", json!(2)),
        (7, "/data/message/toolCallId", json!("other")),
    ] {
        let mut rows = base();
        *rows[index].pointer_mut(pointer).unwrap() = value;
        assert!(validate(rows, false, 0).is_err(), "{pointer}");
    }
    let mut twice = base();
    twice.insert(7, twice[6].clone());
    assert!(validate(twice, false, 0).is_err());
    let mut twice = base();
    twice.insert(8, twice[7].clone());
    assert!(validate(twice, false, 0).is_err());
    let mut advertised = base();
    advertised[5]["data"]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool-call","id":"c","name":"read","arguments":"{}"}));
    assert!(validate(advertised, false, 0).is_err());
}

#[test]
fn exact_not_started_repairs_can_settle_an_advertised_but_unstarted_call() {
    let mut rows = base();
    rows.remove(6);
    rows[6]["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    rows[6]["data"]["message"]["isError"] = json!(true);
    rows[6]["data"]["message"]["id"] = json!("interrupted-tool-result-c-42");
    rows[6]["data"]["message"]["content"] = json!([{"type":"text","text":"The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."}]);
    validate(rows.clone(), false, 0).unwrap();
    let mut wrong = rows.clone();
    wrong[6]["data"]["message"]["content"][0]["text"] = json!("possibly ran");
    assert!(validate(wrong, false, 0).is_err());
    rows[6]["data"]["message"]["id"] = json!("forked-tool-result-c-6");
    rows[6]["data"]["message"]["content"][0]["text"] = json!("Original fork text");
    validate(rows.clone(), false, 0).unwrap();
    rows[6]["data"]["message"]["id"] = json!("forked-tool-result-c-06");
    assert!(validate(rows, false, 0).is_err());
}

#[test]
fn tool_result_replacement_preserves_old_coordinates_within_a_new_turn() {
    let mut rows = base();
    rows.push(event("turn/start", json!({"turn":2})));
    let mut replacement = rows[7].clone();
    replacement["surfaceOp"] = json!({"op":"replace","startSeq":7,"endSeq":7});
    replacement["sourceEventSeqs"] = json!([7]);
    replacement["data"]["message"]["content"][0]["text"] = json!("Refreshed result");
    rows.push(replacement);
    rows.push(event(
        "turn/end",
        json!({"turn":2,"reason":{"kind":"completed"}}),
    ));
    validate(rows.clone(), false, 0).unwrap();
    rows[11]["sourceEventSeqs"] = json!([6]);
    assert!(validate(rows, false, 0).is_err());
}

#[test]
fn turn_step_and_protected_surface_boundaries_are_enforced() {
    for (index, key, value) in [
        (0, "turn", 2),
        (1, "step", 2),
        (8, "step", 2),
        (9, "turn", 2),
    ] {
        let mut rows = base();
        rows[index]["data"][key] = json!(value);
        assert!(validate(rows, false, 0).is_err());
    }
    let mut rows = base();
    rows[3]["surfaceOp"] = json!({"op":"replace","startSeq":2,"endSeq":2});
    rows[3]["sourceEventSeqs"] = json!([2]);
    assert!(validate(rows, false, 0).is_err());
    let mut rows = base();
    rows[2]["surfaceOp"] = Value::Null;
    assert!(validate(rows, false, 0).is_err());
}

#[test]
fn ptc_dispatches_preserve_parent_root_and_structural_arguments() {
    let mut rows = prefix();
    let start = json!({"subCallId":"sub","rootCallId":"root","parentCallId":"root","name":"read","arguments":{"a":1,"b":[2]}});
    rows.push(event("tool/ptc-dispatch-start", start.clone()));
    let mut finish = start;
    finish["content"] = json!([]);
    finish["arguments"] = serde_json::from_str(r#"{"b":[2],"a":1.0}"#).unwrap();
    rows.push(event("tool/ptc-dispatch", finish));
    rows.extend([
        event("step/end", json!({"turn":1,"step":1})),
        event("turn/end", json!({"turn":1})),
    ]);
    validate(rows.clone(), false, 0).unwrap();
    for key in ["rootCallId", "parentCallId", "name"] {
        let mut bad = rows.clone();
        bad[6]["data"][key] = json!("different");
        assert!(validate(bad, false, 0).is_err());
    }
    let mut bad = rows.clone();
    bad[6]["data"]["arguments"]["a"] = json!(3);
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows;
    bad[5]["data"]["parentCallId"] = json!("unstarted-parent");
    assert!(validate(bad, false, 0).is_err());
}

#[test]
fn retries_pair_their_provider_policy_chain_and_single_start() {
    let mut rows = prefix();
    let retry = json!({"turn":1,"step":1,"provider":"mock","policyKey":"policy","retryId":"retry","retry":1});
    rows.push(event("llm/retry", retry.clone()));
    rows.push(event(
        "llm/retry-started",
        json!({"turn":1,"step":1,"retryId":"retry","retry":1}),
    ));
    let mut next = retry;
    next["retry"] = json!(2);
    rows.push(event("llm/retry", next));
    rows.extend([
        event("step/end", json!({"turn":1,"step":1})),
        event("turn/end", json!({"turn":1})),
    ]);
    validate(rows.clone(), false, 0).unwrap();
    for (index, key, value) in [
        (5, "provider", json!("other")),
        (7, "retry", json!(3)),
        (7, "retryId", json!("different")),
        (6, "step", json!(2)),
    ] {
        let mut bad = rows.clone();
        bad[index]["data"][key] = value;
        assert!(validate(bad, false, 0).is_err());
    }
    let mut bad = rows.clone();
    bad.insert(7, bad[6].clone());
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows;
    bad.remove(5);
    assert!(validate(bad, false, 0).is_err());
}

fn compacted() -> Vec<Value> {
    let mut rows = prefix();
    rows.push(user("second"));
    rows.push(event(
        "compaction/start",
        json!({"compactionId":"compact","turn":1}),
    ));
    rows.push(event("compaction/summary",json!({"compactionId":"compact","shadowedRange":{"start":3,"end":5},"shadowedSeqs":[3,5],"summary":[{"type":"text","text":"Summary"}]})));
    let mut checkpoint = message(
        "user/message",
        "checkpoint",
        "user",
        json!({"kind":"compact-checkpoint","compactionId":"compact"}),
        json!([{"type":"text","text":"Summary"}]),
    );
    checkpoint["surfaceOp"] = json!({"op":"replace","startSeq":3,"endSeq":5});
    checkpoint["sourceEventSeqs"] = json!([3, 5, 6, 7]);
    rows.push(checkpoint);
    rows.push(event(
        "compaction/end",
        json!({"compactionId":"compact","turn":1}),
    ));
    rows.extend([
        event("step/end", json!({"turn":1,"step":1})),
        event("turn/end", json!({"turn":1})),
    ]);
    rows
}

#[test]
fn compaction_uses_exact_current_surface_span_and_one_owner() {
    let rows = compacted();
    validate(rows.clone(), false, 0).unwrap();
    let mut bad = rows.clone();
    bad[7]["data"]["shadowedSeqs"] = json!([5, 3]);
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows.clone();
    bad[8]["data"]["source"]["compactionId"] = json!("other");
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows.clone();
    bad[9]["data"]["sourceCommandId"] = Value::Null;
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows.clone();
    bad[7]["data"]["shadowedRange"]["start"] = json!(2);
    bad[7]["data"]["shadowedSeqs"] = json!([2, 3, 5]);
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows;
    bad.remove(9);
    assert!(validate(bad, false, 0).is_err());
}

#[test]
fn unfinished_inherited_compaction_expires_only_at_a_real_seed_boundary() {
    let rows = vec![
        event("turn/start", json!({"turn":1})),
        event("compaction/start", json!({"compactionId":"old","turn":1})),
        event("turn/end", json!({"turn":1})),
        event("session/end-seed", json!({"inherited":true})),
        event("turn/start", json!({"turn":2})),
        event("turn/end", json!({"turn":2})),
    ];
    validate(rows.clone(), true, 3).unwrap();
    assert!(validate(rows[..3].to_vec(), false, 0).is_err());
    let mut bad = rows;
    bad[3]["data"] = Value::Null;
    assert!(validate(bad, true, 3).is_err());
}

#[test]
fn developer_additions_resolve_exactly_one_complete_earlier_definition() {
    let mut rows = prefix();
    let mut developer = message(
        "developer/message",
        "d",
        "developer",
        json!({"kind":"tools-discovery"}),
        json!([{"type":"tool-addition","toolName":"read"}]),
    );
    developer["data"]["headerSeq"] = json!(4);
    rows.push(developer);
    rows.extend([
        event("step/end", json!({"turn":1,"step":1})),
        event("turn/end", json!({"turn":1})),
    ]);
    validate(rows.clone(), false, 0).unwrap();
    let mut bad = rows.clone();
    bad[4]["data"]["header"]["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"read","description":"Duplicate","parameters":{}}));
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows.clone();
    bad[4]["data"]["header"]["tools"][0]["parameters"] = json!(false);
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows;
    bad[5]["data"]["headerSeq"] = json!(3);
    assert!(validate(bad, false, 0).is_err());
}

#[test]
fn title_and_command_references_require_their_recorded_sources() {
    let mut rows = base();
    rows.extend([
        event(
            "session/title",
            json!({"title":"Title","source":{"kind":"generated"},"messageSeqs":[3]}),
        ),
        event("command/run", json!({"commandId":"command"})),
        event(
            "command/done",
            json!({"commandId":"command","kind":"success","sourceEventSeq":3}),
        ),
    ]);
    validate(rows.clone(), false, 0).unwrap();
    for refs in [json!([2]), json!([3, 3]), json!([99])] {
        let mut bad = rows.clone();
        bad[10]["data"]["messageSeqs"] = refs;
        assert!(validate(bad, false, 0).is_err());
    }
    let mut bad = rows.clone();
    bad[12]["data"]["sourceEventSeq"] = json!(11);
    assert!(validate(bad, false, 0).is_err());
    let mut bad = rows;
    bad[12]["data"]["commandId"] = json!("absent");
    assert!(validate(bad, false, 0).is_err());
}

#[test]
fn native_inheritance_catalog_and_delivery_ownership_are_scope_bound() {
    let fact = json!({"version":1,"childId":"child","childCreatedAt":1,"mode":"unknown"});
    let rows = vec![
        event("subagent/catalog", json!({"version":99})),
        event(
            "session-log-deepseek/delivery-accepted",
            json!({"sessionFormatVersion":4,"throughSeq":0,"sessionId":"parent"}),
        ),
        event("session/end-seed", json!({"inherited":true})),
        event("subagent/catalog", fact.clone()),
    ];
    validate(rows.clone(), true, 2).unwrap();
    assert!(validate(rows.clone(), true, 1).is_err());
    let mut bad = rows.clone();
    bad.push(event("subagent/catalog", fact));
    assert!(validate(bad, true, 2).is_err());
    let mut bad = rows;
    bad.push(event(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionFormatVersion":4,"throughSeq":0,"sessionId":"parent"}),
    ));
    assert!(validate(bad, true, 2).is_err());
}

#[test]
fn unknown_ignorable_rows_remain_opaque_and_rejection_poisons_validation() {
    let mut row = event("plugin:future", json!({"content":[{"type":"tool-result"}]}));
    row["ignorable"] = json!(true);
    row["surfaceOp"] = json!({"opaque":true});
    row["sourceEventSeqs"] = json!([-1, "external"]);
    validate(vec![row.clone()], false, 0).unwrap();
    row.as_object_mut().unwrap().remove("ignorable");
    assert!(validate(vec![row], false, 0).is_err());
    let mut validator = V4Validator::new(header(false), 0).unwrap();
    let rows = numbered(vec![event("turn/end", json!({"turn":1}))]);
    assert!(validator.push(&rows[0]).is_err());
    assert!(validator.finish().is_err());
}

fn transform_rows(rows: Vec<Value>, dialect: V3Dialect) -> (V4TransformSummary, Vec<Value>) {
    let mut source_header = header(false);
    source_header["version"] = json!(3);
    let mut stage = V3ToV4Transform::new(source_header, Some(vec![]), Some(0), dialect).unwrap();
    let mut output = vec![];
    for row in numbered(rows) {
        output.extend(stage.push(row).unwrap());
    }
    let (summary, catalog) = stage.finish().unwrap();
    output.extend(catalog);
    (summary, output)
}

#[test]
fn rust_null_compaction_owner_is_preserved_as_extension_without_relaxing_v4_rules() {
    let mut rows = compacted();
    for index in [6, 7, 9] {
        rows[index]["data"]["sourceCommandId"] = Value::Null;
    }
    rows[6]["data"]["plugin:rust-v3:sourceCommandId"] = json!("preexisting extension");
    let (_, released) = transform_rows(rows.clone(), V3Dialect::Released);
    assert!(validate(released, false, 0).is_err());
    let (_, converted) = transform_rows(rows, V3Dialect::Rust);
    validate(converted.clone(), false, 0).unwrap();
    assert!(converted[6]["data"].get("sourceCommandId").is_none());
    assert_eq!(
        converted[6]["data"]["plugin:rust-v3:sourceCommandId"],
        "preexisting extension"
    );
    assert_eq!(
        converted[6]["data"].get("plugin:rust-v3:plugin:rust-v3:sourceCommandId"),
        Some(&Value::Null)
    );
    for index in [7, 9] {
        assert_eq!(
            converted[index]["data"].get("plugin:rust-v3:sourceCommandId"),
            Some(&Value::Null)
        );
    }
    let mut wrong = compacted();
    wrong[6]["data"]["sourceCommandId"] = json!("different-command");
    let (_, converted) = transform_rows(wrong, V3Dialect::Rust);
    assert!(validate(converted, false, 0).is_err());
}

#[test]
fn repeated_compaction_uses_surface_order_even_when_sequence_endpoints_decrease() {
    let mut rows = compacted()[..10].to_vec();
    rows[7]["data"]["shadowedRange"] = json!({"start":3,"end":3});
    rows[7]["data"]["shadowedSeqs"] = json!([3]);
    rows[8]["surfaceOp"] = json!({"op":"replace","startSeq":3,"endSeq":3});
    rows[8]["sourceEventSeqs"] = json!([3, 6, 7]);
    rows.push(event(
        "compaction/start",
        json!({"compactionId":"second","turn":1}),
    ));
    rows.push(event("compaction/summary",json!({"compactionId":"second","shadowedRange":{"start":8,"end":5},"shadowedSeqs":[8,5],"summary":[{"type":"text","text":"Second summary"}]})));
    let mut checkpoint = message(
        "user/message",
        "second-checkpoint",
        "user",
        json!({"kind":"compact-checkpoint","compactionId":"second"}),
        json!([{"type":"text","text":"Second summary"}]),
    );
    checkpoint["surfaceOp"] = json!({"op":"replace","startSeq":8,"endSeq":5});
    checkpoint["sourceEventSeqs"] = json!([8, 5, 10, 11]);
    rows.push(checkpoint);
    rows.extend([
        event("compaction/end", json!({"compactionId":"second","turn":1})),
        event("step/end", json!({"turn":1,"step":1})),
        event("turn/end", json!({"turn":1})),
    ]);
    validate(rows.clone(), false, 0).unwrap();
    let (_, converted) = transform_rows(rows, V3Dialect::Released);
    validate(converted, false, 0).unwrap();
}

#[test]
fn missing_compaction_owner_or_nonboolean_seed_cannot_erase_lifecycle_evidence() {
    validate(vec![event("session/end-seed", json!({}))], false, 0).unwrap();
    assert!(
        validate(
            vec![event(
                "compaction/start",
                json!({"compactionId":"missing-owner"})
            )],
            false,
            0
        )
        .is_err()
    );
    let start = event(
        "compaction/start",
        json!({"compactionId":"manual","turn":null}),
    );
    assert!(
        validate(
            vec![
                start,
                event(
                    "compaction/end",
                    json!({"compactionId":"manual","error":{"code":"FAILED"}})
                )
            ],
            false,
            0
        )
        .is_err()
    );
    for inherited in [Value::Null, json!(false), json!(1), json!("true")] {
        assert!(
            validate(
                vec![event("session/end-seed", json!({"inherited":inherited}))],
                false,
                0
            )
            .is_err()
        );
    }
}
