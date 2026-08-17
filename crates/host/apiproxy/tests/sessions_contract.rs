//! Wire-shape checks for the sessions contract layer (the zod schema's
//! serde counterpart): discriminated unions, optional-field absence, and
//! request roundtrips.

use dsh_host_apiproxy::{
    PromptContentPart, PromptMode, QueueAction, SessionCreateRequest, SessionForkRequest,
    SessionListRequest, SessionPromptRequest, SessionSelectModelRequest, SessionSummary,
};
use dsh_session::session_id;

#[test]
fn prompt_content_parts_discriminate_on_type() {
    let part = PromptContentPart::Image {
        media_type: dsh_attachment::ImageMediaType::Png,
        data: "base64".to_string(),
        name: None,
    };
    let json = serde_json::to_string(&part).expect("serialize");
    assert_eq!(json, r#"{"type":"image","mediaType":"image/png","data":"base64"}"#);
    let back: PromptContentPart = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, part);

    let part = PromptContentPart::Text {
        text: "hi".to_string(),
    };
    let json = serde_json::to_string(&part).expect("serialize");
    assert_eq!(json, r#"{"type":"text","text":"hi"}"#);
    assert!(serde_json::from_str::<PromptContentPart>(r#"{"type":"nope"}"#).is_err());
}

#[test]
fn queue_actions_discriminate_on_kind() {
    let action = QueueAction::Edit {
        content: vec![dsh_llm::ContentBlock::Text {
            text: "v2".to_string(),
        }],
    };
    let json = serde_json::to_string(&action).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["kind"], "edit");
    assert_eq!(parsed["content"][0]["text"], "v2");

    assert_eq!(
        serde_json::to_string(&QueueAction::Remove).expect("serialize"),
        r#"{"kind":"remove"}"#
    );
    assert_eq!(
        serde_json::to_string(&QueueAction::Steer).expect("serialize"),
        r#"{"kind":"steer"}"#
    );
    assert!(serde_json::from_str::<QueueAction>(r#"{"kind":"nope"}"#).is_err());
}

#[test]
fn requests_roundtrip_with_optional_field_absence() {
    let request = SessionCreateRequest {
        workspace_id: None,
        cwd: Some("/proj".to_string()),
        session_id: None,
        agent_preset: Some("default".to_string()),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert_eq!(
        json,
        r#"{"cwd":"/proj","agentPreset":"default"}"#,
        "absent fields stay absent"
    );
    let back: SessionCreateRequest = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, request);

    let prompt = SessionPromptRequest {
        session_id: session_id("s1"),
        mode: PromptMode::Queue,
        content: vec![PromptContentPart::Text {
            text: "hello".to_string(),
        }],
        client_time_zone: Some("Asia/Shanghai".to_string()),
    };
    let json = serde_json::to_string(&prompt).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["sessionId"], "s1");
    assert_eq!(parsed["mode"], "queue");
    assert_eq!(parsed["clientTimeZone"], "Asia/Shanghai");
    let back: SessionPromptRequest = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, prompt);

    let _: SessionListRequest = Default::default();
    let _ = SessionForkRequest {
        session_id: session_id("s1"),
        at_seq: Some(9),
    };
    let _ = SessionSelectModelRequest {
        session_id: session_id("s1"),
        provider: "openai".to_string(),
        model: "gpt-x".to_string(),
        reasoning_effort: None,
    };
    let _ = SessionSummary {
        session_id: session_id("s1"),
        updated_at: 1,
        running: false,
        blank: true,
        parent_session_id: None,
        origin: None,
        cwd: None,
        agent_preset: None,
        projections: None,
    };
}
