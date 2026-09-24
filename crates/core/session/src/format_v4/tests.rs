use super::*;
use crate::SessionEvent;
use serde_json::{Value, json};

fn event(kind: &str, data: Value) -> SessionEvent {
    serde_json::from_value(json!({"type":kind,"seq":7,"time":1234,"data":data})).unwrap()
}

fn wrapped() -> SessionEvent {
    event(
        "tool/result",
        json!({"turn":1,"step":1,"message":{
        "id":"same-result","role":"user","source":{"kind":"tool","callId":"call-1","opaque":{"seq":99}},
        "content":[{"type":"tool-result","toolCallId":"call-1","content":[{"type":"text","text":"原始输出"}],"isError":true}]
    },"error":{"code":"FAILED"}}),
    )
}

#[test]
fn tool_role_preserves_result_identity_content_error_and_envelope() {
    let source = wrapped();
    let target = convert_v3_event_messages(source.clone()).unwrap();
    let result = &target.data["message"];
    assert_eq!(target.seq, source.seq);
    assert_eq!(target.time, source.time);
    assert_eq!(result["role"], "tool");
    assert_eq!(result["toolCallId"], "call-1");
    assert_eq!(result["id"], "same-result");
    assert_eq!(
        result["content"],
        source.data["message"]["content"][0]["content"]
    );
    assert_eq!(result["source"], source.data["message"]["source"]);
    assert_eq!(result["isError"], true);
    assert_eq!(target.data["error"], source.data["error"]);
    validate_v4_message_fields(&target).unwrap();
    assert!(validate_v4_message_fields(&source).is_err());
}

#[test]
fn extension_fields_keep_both_owners_and_original_prefixed_keys() {
    let mut source = wrapped();
    source.data["message"]["isError"] = json!("outer extension");
    source.data["message"]["toolCallId"] = json!("outer identity extension");
    source.data["message"]["same"] = json!(1);
    source.data["message"]["plugin:result:same"] = json!(2);
    source.data["message"]["__proto__"] = json!({"preserve":true});
    source.data["message"]["content"][0]["same"] = json!(3);
    source.data["message"]["content"][0]["constructor"] = json!([4]);
    let target = convert_v3_event_messages(source).unwrap();
    let message = &target.data["message"];
    assert_eq!(message["isError"], true);
    assert_eq!(message["toolCallId"], "call-1");
    assert_eq!(message["plugin:message:isError"], "outer extension");
    assert_eq!(
        message["plugin:message:toolCallId"],
        "outer identity extension"
    );
    assert_eq!(message["plugin:message:same"], 1);
    assert_eq!(message["plugin:message:plugin:result:same"], 2);
    assert_eq!(message["plugin:result:same"], 3);
    assert_eq!(
        message["plugin:message:__proto__"],
        json!({"preserve":true})
    );
    assert_eq!(message["plugin:result:constructor"], json!([4]));
}

#[test]
fn malformed_wrappers_and_unsupported_nested_results_are_refused() {
    for pointer in [
        "/message/id",
        "/message/source/callId",
        "/message/content/0/toolCallId",
    ] {
        let mut source = wrapped();
        *source.data.pointer_mut(pointer).unwrap() = json!("");
        assert!(convert_v3_event_messages(source).is_err(), "{pointer}");
    }
    for value in [Value::Null, json!("true"), json!(0)] {
        let mut source = wrapped();
        source.data["message"]["content"][0]["isError"] = value;
        assert!(convert_v3_event_messages(source).is_err());
    }
    let mut source = wrapped();
    source.data["message"]["content"][0]["content"] = json!([{"type":"tool-result","content":[]}]);
    assert!(
        convert_v3_event_messages(source)
            .unwrap_err()
            .contains("nested")
    );
    let mut source = wrapped();
    source.data["message"]["content"] = json!([]);
    assert!(convert_v3_event_messages(source).is_err());
}

#[test]
fn optional_error_and_empty_content_are_preserved_without_inventing_fields() {
    let mut source = wrapped();
    source.data.as_object_mut().unwrap().remove("error");
    let wrapper = source.data["message"]["content"][0]
        .as_object_mut()
        .unwrap();
    wrapper.remove("isError");
    wrapper.insert("content".into(), json!([]));
    let target = convert_v3_event_messages(source).unwrap();
    assert_eq!(target.data["message"]["content"], json!([]));
    assert!(target.data["message"].get("isError").is_none());
    validate_v4_message_fields(&target).unwrap();
}

#[test]
fn known_and_unknown_producers_are_role_sensitive_and_lossless() {
    for (plugin, role, expected) in [
        ("@deepseek-ai/dsh-system-prompt", "system", "system-prompt"),
        ("@deepseek-ai/dsh-system-prompt", "user", "runtime-context"),
        ("compact", "user", "compact-checkpoint"),
        ("tools-code-mode", "user", "ptc-mode"),
        ("tools-ptc", "user", "ptc-mode"),
        ("dsh-compaction-basic", "user", "compact-basic"),
        ("goal", "user", "goal"),
        ("skill-catalog", "user", "skill-catalog"),
        ("third-party", "user", "plugin:third-party"),
        ("plugin:third-party", "user", "plugin:plugin:third-party"),
    ] {
        let message = json!({"id":"stable","role":role,"source":{"kind":"plugin","plugin":plugin,"form":"snapshot","extra":{"kind":"plugin","plugin":"opaque"}},"content":[]});
        let source = if role == "system" {
            event(
                "system/message",
                json!({"turn":1,"step":1,"message":message}),
            )
        } else {
            event("user/message", message)
        };
        let target = convert_v3_event_messages(source).unwrap();
        let message = if role == "system" {
            &target.data["message"]
        } else {
            &target.data
        };
        assert_eq!(message["source"]["kind"], expected);
        assert!(message["source"].get("plugin").is_none());
        assert_eq!(
            message["source"]["extra"],
            json!({"kind":"plugin","plugin":"opaque"})
        );
        validate_v4_message_fields(&target).unwrap();
    }
    let source = event(
        "user/message",
        json!({"id":"u","role":"user","source":{"kind":"custom-producer","future":true},"content":[]}),
    );
    assert_eq!(convert_v3_event_messages(source.clone()).unwrap(), source);
}

#[test]
fn conversion_traverses_declared_messages_but_not_arguments_or_extension_data() {
    let opaque = json!({"kind":"plugin","plugin":"compact","type":"tool-result","seq":500});
    let message = json!({"id":"m","role":"user","source":{"kind":"plugin","plugin":"session-reference","captured":opaque},"content":[{"type":"plugin:future","payload":opaque},{"type":"tool-call","id":"call","name":"tool","arguments":opaque.to_string()}]});
    for (kind, key) in [
        ("agent/inbox/spliced", "inserted"),
        ("session/title-llm-request", "messages"),
    ] {
        let mut data = json!({"opaque":opaque});
        data[key] = json!([message]);
        let converted = convert_v3_event_messages(event(kind, data)).unwrap();
        assert_eq!(
            converted.data[key][0]["source"]["kind"],
            "session-reference"
        );
        assert_eq!(
            converted.data[key][0]["content"][0]["type"],
            "plugin:plugin:future"
        );
        assert_eq!(converted.data[key][0]["content"][0]["payload"], opaque);
        assert_eq!(
            converted.data[key][0]["content"][1]["arguments"],
            opaque.to_string()
        );
        assert_eq!(converted.data["opaque"], opaque);
    }
    let foreign = event("plugin:opaque", json!({"message":message}));
    assert_eq!(convert_v3_event_messages(foreign.clone()).unwrap(), foreign);
}

#[test]
fn content_and_stream_extensions_keep_order_indices_and_other_fields() {
    let stream = json!([{"type":"chunk","chunk":{"type":"block-start","index":9,"blockType":"unknown","custom":{"type":"tool-result"}}},{"type":"chunk","chunk":{"type":"block-end","index":9,"block":{"type":"unknown","data":7}}}]);
    let target =
        convert_v3_event_messages(event("assistant/attempt", json!({"stream":stream}))).unwrap();
    assert_eq!(
        target.data["stream"][0]["chunk"]["blockType"],
        "plugin:unknown"
    );
    assert_eq!(
        target.data["stream"][0]["chunk"]["custom"]["type"],
        "tool-result"
    );
    assert_eq!(
        target.data["stream"][1]["chunk"]["block"]["type"],
        "plugin:unknown"
    );
    assert_eq!(target.data["stream"][1]["chunk"]["index"], 9);
    let target = convert_v3_event_messages(event(
        "assistant/chunk",
        json!({"chunk":stream[0]["chunk"]}),
    ))
    .unwrap();
    assert_eq!(target.data["chunk"]["blockType"], "plugin:unknown");
    let target=convert_v3_event_messages(event("compaction/summary",json!({"summary":[{"type":"custom","value":1}],"rawOutput":[{"type":"text","text":"same"}]}))).unwrap();
    assert_eq!(target.data["summary"][0]["type"], "plugin:custom");
    assert_eq!(target.data["rawOutput"][0]["text"], "same");
}

#[test]
fn v4_rejects_retired_fields_even_when_other_rows_would_be_recoverable() {
    for header in [json!({"system":"old"}), json!({"system":null})] {
        assert!(
            validate_v4_message_fields(&event("request/header", json!({"header":header}))).is_err()
        );
    }
    for kind in ["tool/code-dispatch", "tool/code-dispatch-start"] {
        let mut source = event(kind, json!({"opaque":true}));
        assert!(validate_v4_message_fields(&source).is_err());
        source.ignorable = Some(true);
        validate_v4_message_fields(&source).unwrap();
    }
    let invalid = event(
        "request/header",
        json!({"header":{"tools":[{"name":"later","deferLoading":false}]}}),
    );
    assert!(convert_v3_event_messages(invalid).is_err());
    let mut target = convert_v3_event_messages(wrapped()).unwrap();
    target.data["message"]["isError"] = json!(false);
    assert!(validate_v4_message_fields(&target).is_err());
    target.data["message"]["source"] = json!({"kind":"plugin","plugin":"old"});
    assert!(validate_v4_message_fields(&target).is_err());
}
