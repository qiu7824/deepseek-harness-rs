use super::*;
use serde_json::{Value, json};

fn header(seeded: bool) -> Value {
    let mut value =
        json!({"version":3,"id":"parent","createdAt":1,"isSeeded":seeded,"delegationDepth":0});
    if seeded {
        value["parentSession"] = json!("ancestor");
    }
    value
}
fn row(kind: &str, data: Value) -> Value {
    json!({"type":kind,"data":data})
}
fn numbered(rows: Vec<Value>) -> Vec<Value> {
    rows.into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r["seq"] = json!(i);
            r["time"] = json!(i + 10);
            r
        })
        .collect()
}
fn user(id: &str) -> Value {
    json!({"id":id,"role":"user","source":{"kind":"user"},"content":[{"type":"text","text":id}]})
}
fn prefix() -> Vec<Value> {
    vec![
        row("turn/start", json!({"turn":1})),
        row("step/start", json!({"turn":1,"step":1})),
        row("step/end", json!({"turn":1,"step":1})),
        row(
            "agent/inbox/spliced",
            json!({"target":"next-turn","inserted":[user("next")]}),
        ),
    ]
}
fn migrate(
    rows: Vec<Value>,
    children: Vec<Value>,
    seeded: bool,
    cut: Option<u64>,
    dialect: V3Dialect,
) -> Result<(V4TransformSummary, Vec<Value>), String> {
    let mut stage = V3ToV4Transform::new(header(seeded), Some(children), cut, dialect)?;
    let mut out = vec![];
    for row in numbered(rows) {
        out.extend(stage.push(row)?)
    }
    let (summary, catalog) = stage.finish()?;
    out.extend(catalog);
    Ok((summary, out))
}
fn child(id: &str, created: u64) -> Value {
    json!({"childId":id,"childCreatedAt":created,"descriptorCount":1,"descriptor":{"version":3,"provider":"spawn","mode":"continuable","label":"child task"}})
}
fn fact(id: &str, created: u64) -> Value {
    json!({"version":0,"childId":id,"childCreatedAt":created,"mode":"continuable","label":"child task"})
}

#[test]
fn child_evidence_is_explicit_and_rust_header_compatibility_is_not_global() {
    assert!(V3ToV4Transform::new(header(false), None, None, V3Dialect::Released).is_err());
    let mut value = header(false);
    value.as_object_mut().unwrap().remove("delegationDepth");
    assert!(V3ToV4Transform::new(value.clone(), Some(vec![]), None, V3Dialect::Released).is_err());
    let (summary, events) = V3ToV4Transform::new(value, Some(vec![]), None, V3Dialect::Rust)
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(summary.header["version"], 4);
    assert_eq!(summary.header["delegationDepth"], 0);
    assert!(events.is_empty());
    for bad in [Value::Null, json!([]), json!("header")] {
        assert!(V3ToV4Transform::new(bad, Some(vec![]), None, V3Dialect::Rust).is_err());
    }
    let mut value = header(false);
    value["futureField"] = json!(true);
    assert!(V3ToV4Transform::new(value, Some(vec![]), None, V3Dialect::Rust).is_err());
}

#[test]
fn physical_header_framing_is_decoded_before_logical_migration() {
    let legacy =
        json!({"type":"session","version":3,"id":"recorded","createdAt":1,"delegationDepth":0});
    let (logical, cut) = decode_v3_header(legacy.clone(), V3Dialect::Rust).unwrap();
    assert_eq!(logical["isSeeded"], false);
    assert_eq!(cut, Some(0));
    assert!(logical.get("type").is_none());
    assert!(decode_v3_header(legacy.clone(), V3Dialect::Released).is_err());
    let mut seeded = legacy;
    seeded["seedLength"] = json!(8);
    let (logical, cut) = decode_v3_header(seeded.clone(), V3Dialect::Rust).unwrap();
    assert_eq!(logical["isSeeded"], true);
    assert_eq!(cut, Some(8));
    assert!(logical.get("seedLength").is_none());
    seeded["isSeeded"] = json!(true);
    assert!(decode_v3_header(seeded, V3Dialect::Rust).is_err());
    let mut released = header(true);
    released["type"] = json!("session");
    assert_eq!(
        decode_v3_header(released, V3Dialect::Released).unwrap().1,
        None
    );
}

#[test]
fn rust_physical_inherited_cut_marks_only_its_own_empty_boundary() {
    let (summary, rows) = migrate(vec![row("feedback/record", json!({})), row("session/end-seed", json!({})), row("session/end-seed", json!({}))], vec![], true, Some(1), V3Dialect::Rust).unwrap();
    assert_eq!(summary.inherited_event_count, 1);
    assert_eq!(rows[1]["data"], json!({"inherited":true}));
    assert_eq!(rows[2]["data"], json!({}));
    let mut validator = V4Validator::new(summary.header, summary.inherited_event_count).unwrap();
    for row in rows { validator.push(&row).unwrap(); }
    assert_eq!(validator.finish().unwrap().inherited_event_count, 1);
    assert!(migrate(vec![row("session/end-seed", json!({}))], vec![], true, Some(9), V3Dialect::Rust).is_err());
}

#[test]
fn restart_insertion_maps_every_owned_reference_without_touching_captured_coordinates() {
    let mut rows = prefix();
    rows.push(row("turn/start", json!({"turn":2})));
    rows.push(row("user/message", user("after")));
    rows.push(row(
        "session/title",
        json!({"title":"same","messageSeqs":[5],"source":{"kind":"generated"}}),
    ));
    rows.push(row(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionId":"parent","sessionFormatVersion":3,"throughSeq":6}),
    ));
    let mut replacement = row(
        "user/message",
        json!({"id":"replacement","role":"user","source":{"kind":"plugin","plugin":"session-reference","throughSeq":5,"capturedFormatVersion":3},"content":[]}),
    );
    replacement["surfaceOp"] = json!({"op":"replace","startSeq":5,"endSeq":5});
    replacement["sourceEventSeqs"] = json!([5, 6]);
    rows.push(replacement);
    rows.push(row(
        "image/offload",
        json!({"targets":[{"seq":5,"attachmentId":"same","opaque":{"seq":5}}]}),
    ));
    rows.push(row("compaction/summary",json!({"summary":[{"type":"text","text":"same"}],"shadowedRange":{"start":5,"end":8},"shadowedSeqs":[5,8]})));
    rows.push(row(
        "command/done",
        json!({"sourceEventSeq":8,"result":{"sourceEventSeq":8}}),
    ));
    let original = rows.clone();
    let (summary, out) = migrate(rows, vec![], false, Some(0), V3Dialect::Released).unwrap();
    assert_eq!(out.len(), original.len() + 1);
    assert_eq!(
        out[4],
        json!({"type":"turn/end","seq":4,"time":14,"data":{"turn":1,"reason":{"kind":"interrupted"}}})
    );
    assert_eq!(summary.source_offsets[4], 5);
    assert_eq!(summary.source_cuts[4], 4);
    assert_eq!(summary.source_cuts[5], 6);
    assert_eq!(out[7]["data"]["messageSeqs"], json!([6]));
    assert_eq!(out[8]["data"]["throughSeq"], 6);
    assert_eq!(out[8]["data"]["sessionFormatVersion"], 3);
    assert_eq!(
        out[9]["surfaceOp"],
        json!({"op":"replace","startSeq":6,"endSeq":6})
    );
    assert_eq!(out[9]["sourceEventSeqs"], json!([6, 7]));
    assert_eq!(out[9]["data"]["source"]["throughSeq"], 5);
    assert_eq!(out[9]["data"]["source"]["kind"], "session-reference");
    assert_eq!(out[10]["data"]["targets"][0]["seq"], 6);
    assert_eq!(out[10]["data"]["targets"][0]["opaque"]["seq"], 5);
    assert_eq!(out[11]["data"]["shadowedSeqs"], json!([6, 9]));
    assert_eq!(out[11]["data"]["shadowedRange"], json!({"start":6,"end":9}));
    assert_eq!(out[12]["data"]["sourceEventSeq"], 9);
    assert_eq!(out[12]["data"]["result"]["sourceEventSeq"], 8);
    assert_eq!(out.iter().enumerate().all(|(i, row)| row["seq"] == i), true);
    assert_eq!(summary.event_count, out.len() as u64);
}

#[test]
fn only_an_adjacent_nonempty_next_turn_splice_can_evidence_an_inserted_end() {
    for changed in ["nonadjacent", "empty", "next-step", "open-step", "skipped"] {
        let mut rows = prefix();
        match changed {
            "nonadjacent" => rows.push(row("feedback/record", json!({}))),
            "empty" => rows[3]["data"]["inserted"] = json!([]),
            "next-step" => rows[3]["data"]["target"] = json!("next-step"),
            "open-step" => {
                rows.remove(2);
            }
            _ => {}
        }
        rows.push(row(
            "turn/start",
            json!({"turn":if changed=="skipped"{3}else{2}}),
        ));
        let (_, out) = migrate(rows, vec![], false, None, V3Dialect::Released).unwrap();
        assert!(!out.iter().any(|e| e["type"] == "turn/end"), "{changed}");
        // Complete lifecycle validation, rather than conversion, rejects these transitions.
    }
    let (_, open) = migrate(prefix(), vec![], false, None, V3Dialect::Released).unwrap();
    assert_eq!(open.len(), 4);
}

#[test]
fn inherited_cut_uses_the_last_marker_and_catalogs_belong_to_the_own_suffix() {
    let rows = vec![
        row("subagent/catalog", json!({"version":99})),
        row("session/end-seed", json!({"inherited":true})),
        row("subagent/catalog", json!({"version":99})),
        row("session/end-seed", json!({"inherited":true})),
    ];
    let (summary, out) = migrate(
        rows.clone(),
        vec![child("own", 2)],
        true,
        Some(3),
        V3Dialect::Released,
    )
    .unwrap();
    assert_eq!(summary.inherited_event_count, 3);
    assert_eq!(out.len(), 5);
    assert_eq!(out[4]["data"], fact("own", 2));
    assert_eq!(summary.source_cuts.last(), Some(&4));
    assert_eq!(summary.event_count, 5);
    assert!(migrate(rows, vec![], true, Some(1), V3Dialect::Released).is_err());
    assert!(migrate(vec![], vec![], true, None, V3Dialect::Released).is_err());
    assert!(
        migrate(
            vec![row("session/end-seed", json!({"inherited":true}))],
            vec![],
            false,
            None,
            V3Dialect::Released
        )
        .is_err()
    );
    let mut rows = prefix();
    rows.push(row("turn/start", json!({"turn":2})));
    rows.push(row("session/end-seed", json!({"inherited":true})));
    let (summary, _) = migrate(rows, vec![], true, Some(5), V3Dialect::Released).unwrap();
    assert_eq!(summary.inherited_event_count, 6);
}

#[test]
fn child_catalogs_are_deterministic_deduplicated_and_do_not_guess_unknown_modes() {
    let mut old = child("legacy", 2);
    old["descriptor"] = json!({"version":1,"provider":"spawn","label":"child task"});
    let unknown = json!({"childId":"unknown","childCreatedAt":3,"descriptorCount":2,"descriptor":{"version":3,"mode":"continuable"}});
    let (_, out) = migrate(
        vec![],
        vec![child("z", 4), old.clone(), child("a", 4), unknown, old],
        false,
        None,
        V3Dialect::Released,
    )
    .unwrap();
    assert_eq!(
        out.iter()
            .map(|r| r["data"]["childId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["legacy", "unknown", "a", "z"]
    );
    assert_eq!(out[0]["data"]["mode"], "continuable");
    assert_eq!(
        out[1]["data"],
        json!({"version":1,"childId":"unknown","childCreatedAt":3,"mode":"unknown"})
    );
    let (_, out) = migrate(
        vec![row("subagent/catalog", fact("legacy", 2))],
        vec![child("legacy", 2)],
        false,
        None,
        V3Dialect::Released,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
}

#[test]
fn conflicting_or_duplicate_own_child_identity_prevents_finish() {
    for mut incoming in [child("c", 3), child("c", 2)] {
        if incoming["childCreatedAt"] == 2 {
            incoming["descriptor"]["label"] = json!("changed label");
        }
        assert!(
            migrate(
                vec![row("subagent/catalog", fact("c", 2))],
                vec![incoming],
                false,
                None,
                V3Dialect::Released
            )
            .is_err()
        );
    }
    assert!(
        migrate(
            vec![
                row("subagent/catalog", fact("c", 2)),
                row("subagent/catalog", fact("c", 2))
            ],
            vec![],
            false,
            None,
            V3Dialect::Released
        )
        .is_err()
    );
    let mut bad = child("c", 2);
    bad["descriptor"]["provider"] = json!(false);
    assert!(migrate(vec![], vec![bad], false, None, V3Dialect::Released).is_err());
}

#[test]
fn historical_child_discovery_excludes_inherited_descriptors() {
    let child_header =
        json!({"id":"child","createdAt":20,"origin":"subagent","parentSession":"parent"});
    let rows = numbered(vec![
        row(
            "subagent/descriptor",
            json!({"version":3,"provider":"wrong","mode":"one-shot"}),
        ),
        row("session/end-seed", json!({"inherited":true})),
        row(
            "subagent/descriptor",
            json!({"version":1,"provider":"spawn","label":"own"}),
        ),
    ]);
    let source = historical_child_catalog_source(&child_header, 1, rows.iter()).unwrap();
    assert_eq!(source["descriptorCount"], 1);
    assert_eq!(source["descriptor"]["label"], "own");
    assert!(historical_child_catalog_source(&child_header, 8, rows.iter()).is_err());
    assert!(historical_child_catalog_source(&json!({"id":"child"}), 0, rows.iter()).is_err());
}

#[test]
fn generation_qualified_delivery_coordinates_are_not_remapped() {
    let delivery = row(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionId":"ancestor","sessionFormatVersion":3,"throughSeq":0}),
    );
    let rows = vec![
        row("feedback/record", json!({})),
        delivery.clone(),
        row("session/end-seed", json!({"inherited":true})),
    ];
    assert!(migrate(rows, vec![], true, Some(2), V3Dialect::Released).is_ok());
    assert!(
        migrate(
            vec![row("feedback/record", json!({})), delivery.clone()],
            vec![],
            false,
            None,
            V3Dialect::Released
        )
        .is_err()
    );
    let mut future = delivery;
    future["data"]["sessionFormatVersion"] = json!(4);
    assert!(
        migrate(
            vec![row("feedback/record", json!({})), future],
            vec![],
            false,
            None,
            V3Dialect::Released
        )
        .is_err()
    );
}

#[test]
fn unknown_ignorable_rows_preserve_uninterpreted_envelope_fields() {
    let mut rows = prefix();
    rows.push(row("turn/start", json!({"turn":2})));
    let mut opaque = row(
        "developer/message",
        json!({"source":{"kind":"plugin"},"meaning":"not a native V4 developer event"}),
    );
    opaque["ignorable"] = json!(true);
    opaque["surfaceOp"] = json!({"opaque":true});
    opaque["sourceEventSeqs"] = json!([-1, "foreign"]);
    rows.push(opaque.clone());
    let (_, out) = migrate(rows, vec![], false, None, V3Dialect::Released).unwrap();
    let row = &out[6];
    assert_eq!(row["type"], "plugin:developer/message");
    assert_eq!(row["data"], opaque["data"]);
    assert_eq!(row["surfaceOp"], opaque["surfaceOp"]);
    assert_eq!(row["sourceEventSeqs"], opaque["sourceEventSeqs"]);
}

#[test]
fn refused_rows_poison_the_stage_and_cannot_be_skipped_before_publication() {
    for bad in [
        row("future/required", json!({})),
        row("session/title", json!({"messageSeqs":[0]})),
    ] {
        let mut stage =
            V3ToV4Transform::new(header(false), Some(vec![]), None, V3Dialect::Released).unwrap();
        assert!(stage.push(numbered(vec![bad]).remove(0)).is_err());
        assert!(
            stage
                .push(numbered(vec![row("feedback/record", json!({}))]).remove(0))
                .is_err()
        );
        assert!(stage.finish().is_err());
    }
    let mut stage =
        V3ToV4Transform::new(header(false), Some(vec![]), None, V3Dialect::Released).unwrap();
    assert!(
        stage
            .push(json!({"type":"feedback/record","seq":1,"time":1,"data":null}))
            .is_err()
    );
}

#[test]
fn rust_wire_aliases_are_converted_only_under_explicit_rust_authority() {
    let mut replace = row("user/message", user("replacement"));
    replace["surfaceOp"] = json!({"op":"replace","start":0,"end":0});
    let rows = vec![
        row("user/message", user("first")),
        replace,
        row(
            "tool/code-dispatch",
            json!({"content":[{"type":"text","text":"same"}]}),
        ),
        row("request/phase", json!({"opaque":"same"})),
    ];
    assert!(migrate(rows.clone(), vec![], false, None, V3Dialect::Released).is_err());
    let (_, out) = migrate(rows, vec![], false, None, V3Dialect::Rust).unwrap();
    assert_eq!(
        out[1]["surfaceOp"],
        json!({"op":"replace","startSeq":0,"endSeq":0})
    );
    assert_eq!(out[2]["type"], "tool/ptc-dispatch");
    assert_eq!(out[3]["type"], "request/phase");
}

#[test]
fn native_child_descriptor_generations_preserve_catalog_identity() {
    for version in [4, 5] {
        let mut source=child("native-child",4);
        source["descriptor"]["version"]=json!(version);
        let (_,out)=migrate(vec![],vec![source],false,None,V3Dialect::Released).unwrap();
        assert_eq!(out[0]["data"]["mode"],"continuable");
    }
}
