use super::*;
use serde_json::json;

fn producer(plugin: &str) -> MessageSource {
    MessageSource::Plugin {
        plugin: plugin.into(),
        form: Some(ContextForm::Notice),
        summary: Some("A bounded update".into()),
        sections: None,
        compaction_id: None,
        source_command_id: None,
    }
}

#[test]
fn tool_results_use_flat_content_and_message_level_correlation() {
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id: call_id("tool-1"),
        content: vec![ContentBlock::Text {
            text: "result".into(),
        }],
        is_error: true,
    });
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["role"], "tool");
    assert_eq!(value["toolCallId"], "tool-1");
    assert_eq!(value["source"], json!({"kind":"tool","callId":"tool-1"}));
    assert_eq!(value["content"], json!([{"type":"text","text":"result"}]));
    assert_eq!(value["isError"], true);
    assert_eq!(serde_json::from_value::<Message>(value).unwrap(), message);
}

#[test]
fn producer_owned_sources_roundtrip_without_plugin_wrappers() {
    let message = create_user_message(vec![], producer("custom-producer"));
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["source"]["kind"], "plugin:custom-producer");
    assert!(value["source"].get("plugin").is_none());
    assert_eq!(serde_json::from_value::<Message>(value).unwrap(), message);
    let unknown = json!({"kind":"plugin:future","opaque":{"nested":[1,null,"x"]}});
    let source: MessageSource = serde_json::from_value(unknown.clone()).unwrap();
    assert_eq!(serde_json::to_value(source).unwrap(), unknown);
}

#[test]
fn system_and_runtime_context_have_distinct_producers() {
    let system = create_message(
        Role::System,
        vec![],
        producer("@deepseek-ai/dsh-system-prompt"),
    );
    let runtime = create_user_message(vec![], producer("@deepseek-ai/dsh-system-prompt"));
    assert_eq!(system.source.kind(), "system-prompt");
    assert_eq!(runtime.source.kind(), "runtime-context");
    assert_eq!(
        runtime.source.plugin_name(),
        Some("@deepseek-ai/dsh-system-prompt")
    );
    assert!(
        serde_json::to_value(runtime)
            .unwrap()
            .get("toolCallId")
            .is_none()
    );
}

#[test]
fn native_blocks_and_extensions_retain_payload_and_reserved_markers() {
    for value in [
        json!({"type":"file","attachment":{"attachmentId":"f1","name":"report.docx","bytes":42}}),
        json!({"type":"image","attachment":{"attachmentId":"i1"},"offloaded":true}),
        json!({"type":"tool-addition","toolName":"read"}),
        json!({"type":"tool-removal","toolName":"write"}),
        json!({"type":"plugin:custom","opaque":{"x":[1,null,"payload"]}}),
    ] {
        let block: ContentBlock = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(block).unwrap(), value);
    }
    for value in [
        json!({"type":"text","text":42}),
        json!({"type":"unknown","text":"x"}),
        json!({"type":"image","attachment":{"attachmentId":"i1"},"offloaded":false}),
    ] {
        assert!(serde_json::from_value::<ContentBlock>(value).is_err());
    }
}

#[test]
fn native_tool_images_participate_in_request_limits_without_mutating_the_log() {
    let image = |id: &str, offloaded| {
        serde_json::from_value::<ContentBlock>(json!({
            "type":"image","attachment":{"attachmentId":id,"bytes":100},
            "offloaded":offloaded
        }))
        .unwrap()
    };
    let retained: ContentBlock = serde_json::from_value(
        json!({"type":"image","attachment":{"attachmentId":"live","bytes":100}}),
    )
    .unwrap();
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id: call_id("image-call"),
        content: vec![image("old", true), retained],
        is_error: false,
    });
    let projected = offload_request_images(std::slice::from_ref(&message), Some(0));
    assert!(matches!(
        projected[0].content[0],
        ContentBlock::Image {
            offloaded: Some(true),
            ..
        }
    ));
    assert!(matches!(projected[0].content[1], ContentBlock::Text { .. }));
    assert!(!content_has_image(&projected[0].content));
    assert!(content_has_image(&message.content));
    assert_eq!(projected[0].tool_call_id, message.tool_call_id);
}
