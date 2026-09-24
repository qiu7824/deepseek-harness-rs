use super::*;
use serde_json::{Value, json};

fn developer() -> Value {
    json!({"type":"developer/message","seq":2,"time":2,"data":{"turn":1,"step":1,"headerSeq":1,"message":{"id":"developer","role":"developer","source":{"kind":"tools-discovery"},"content":[{"type":"tool-addition","toolName":"read"}]}}})
}
fn tool_result() -> Value {
    json!({"type":"tool/result","seq":3,"time":3,"surfaceOp":"append","data":{"turn":1,"step":1,"error":{"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"},"message":{"id":"forked-tool-result-call-3","role":"tool","toolCallId":"call","isError":true,"source":{"kind":"tool","callId":"call"},"content":[{"type":"text","text":"Preserved fork-specific text"}]}}})
}

#[test]
fn developer_changes_require_their_role_and_header_reference_without_inline_schema() {
    let valid = developer();
    validate_v4_row_fields(&valid).unwrap();
    for pointer in [
        "/data/headerSeq",
        "/data/turn",
        "/data/step",
        "/data/message/id",
        "/data/message/content/0/toolName",
    ] {
        let mut bad = valid.clone();
        *bad.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(admit_v4_structural_fields(&bad).is_err(), "{pointer}");
    }
    let mut inline = valid.clone();
    inline["data"]["message"]["content"][0]["tool"] = json!({"name":"read"});
    assert!(admit_v4_structural_fields(&inline).is_err());
    let mut ordinary = valid.clone();
    ordinary["type"] = json!("assistant/message");
    assert!(admit_v4_structural_fields(&ordinary).is_err());
    let mut removal = valid.clone();
    removal["data"]["message"]["content"][0]["type"] = json!("tool-removal");
    assert!(admit_v4_structural_fields(&removal).is_err());
    removal["data"].as_object_mut().unwrap().remove("headerSeq");
    validate_v4_row_fields(&removal).unwrap();
}

#[test]
fn developer_role_cannot_hide_in_inbox_or_title_messages() {
    for (kind, key) in [
        ("agent/inbox/spliced", "inserted"),
        ("session/title-llm-request", "messages"),
    ] {
        let mut data = json!({});
        data[key] = json!([developer()["data"]["message"]]);
        assert!(admit_v4_structural_fields(&json!({"type":kind,"data":data})).is_err());
    }
    let opaque = json!({"type":"user/message","data":{"id":"u","role":"user","source":{"kind":"user"},"content":[{"type":"plugin:tool-addition","payload":{"type":"tool-addition","tool":false}}]}});
    validate_v4_row_fields(&opaque).unwrap();
}

#[test]
fn deferred_tool_schema_flag_is_literal_true_and_opaque_parameters_are_not_traversed() {
    let mut row = json!({"type":"request/header","data":{"header":{"tools":[{"name":"read","description":"Read","parameters":{"type":"object","properties":{"deferLoading":{"const":false}}},"deferLoading":true}]}}});
    admit_v4_structural_fields(&row).unwrap();
    for value in [json!(false), json!("true"), json!(1), Value::Null] {
        row["data"]["header"]["tools"][0]["deferLoading"] = value;
        assert!(admit_v4_structural_fields(&row).is_err());
    }
    row["data"]["header"]["tools"][0]
        .as_object_mut()
        .unwrap()
        .remove("deferLoading");
    admit_v4_structural_fields(&row).unwrap();
}

#[test]
fn owned_fields_remain_hard_refusals_while_ordinary_corruption_reaches_recovery() {
    let mut row = json!({"type":"user/message","data":{"id":false,"role":"user","source":{"kind":"user"},"content":false}});
    admit_v4_structural_fields(&row).unwrap();
    assert!(validate_v4_row_fields(&row).is_err());
    row["data"]["source"] = json!({"kind":"plugin","plugin":"old"});
    assert!(admit_v4_structural_fields(&row).is_err());
    let mut critical = developer();
    critical["ignorable"] = json!(true);
    critical["data"]["step"] = json!(0);
    assert!(admit_v4_structural_fields(&critical).is_err());
}

#[test]
fn system_image_and_known_block_fields_are_checked_before_recovery() {
    let mut row = json!({"type":"system/message","data":{"turn":1,"step":1,"message":{"id":"system","role":"system","source":{"kind":"system-prompt"},"content":[{"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":0,"width":1,"height":2}}]}}});
    admit_v4_structural_fields(&row).unwrap();
    let valid = row.clone();
    for pointer in [
        "/data/turn",
        "/data/message/content/0/attachment/width",
        "/data/message/content/0/attachment/height",
    ] {
        let mut bad = valid.clone();
        *bad.pointer_mut(pointer).unwrap() = json!(0);
        assert!(admit_v4_structural_fields(&bad).is_err());
    }
    row["data"]["message"]["content"][0]["attachment"]["mediaType"] = json!("image/tiff");
    assert!(admit_v4_structural_fields(&row).is_err());
    row["data"]["message"]["content"] =
        json!([{"type":"tool-call","id":"c","name":"tool","arguments":"not valid JSON"}]);
    admit_v4_structural_fields(&row).unwrap();
    row["data"]["message"]["content"][0]["arguments"] = json!({});
    assert!(admit_v4_structural_fields(&row).is_err());
}

#[test]
fn fork_result_identity_tracks_append_and_exact_original_replacement() {
    let mut row = tool_result();
    validate_v4_row_fields(&row).unwrap();
    row["seq"] = json!(5);
    assert!(admit_v4_structural_fields(&row).is_err());
    row["surfaceOp"] = json!({"op":"replace","startSeq":3,"endSeq":3});
    row["sourceEventSeqs"] = json!([3]);
    validate_v4_row_fields(&row).unwrap();
    row["sourceEventSeqs"] = json!([2]);
    assert!(admit_v4_structural_fields(&row).is_err());
    for suffix in ["03", "-3", "3.0", "9007199254740992"] {
        let mut bad = tool_result();
        bad["data"]["message"]["id"] = json!(format!("forked-tool-result-call-{suffix}"));
        assert!(admit_v4_structural_fields(&bad).is_err());
    }
}

#[test]
fn retired_stream_blocks_and_changes_are_rejected_without_interpreting_extension_payloads() {
    for kind in ["tool-result", "tool-addition", "tool-removal"] {
        let row = json!({"type":"assistant/attempt","data":{"stream":[{"type":"chunk","chunk":{"type":"block-start","index":0,"blockType":kind}}]}});
        assert!(admit_v4_structural_fields(&row).is_err());
        let row = json!({"type":"assistant/chunk","data":{"chunk":{"type":"block-end","index":0,"block":{"type":kind,"toolName":"read"}}}});
        assert!(admit_v4_structural_fields(&row).is_err());
    }
    let row = json!({"type":"assistant/chunk","data":{"chunk":{"type":"block-end","index":0,"block":{"type":"plugin:change","payload":{"type":"tool-result"}}}}});
    admit_v4_structural_fields(&row).unwrap();
}
