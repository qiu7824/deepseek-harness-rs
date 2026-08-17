//! Rust port of the frame-union behaviors of
//! `packages/host/apiproxy/tests/rpc-schemas.spec.ts` / events schema:
//! MuxFrame and HostFrame discriminate on `type` and roundtrip.

use dsh_host_apiproxy::{
    EmptyDetails, HostFrame, MuxFrame, QuestionOutcome, QueuedInboxItem, QueuedInboxPlacement,
    RpcError, RpcErrorBody, rpc_id,
};
use dsh_llm::{Message, MessageSource, Role, message_id};
use dsh_session::{SessionEvent, session_id};

#[test]
fn mux_frames_discriminate_on_type_and_roundtrip() {
    let frame = MuxFrame::SessionSubscribed {
        session_id: session_id("s1"),
        last_seq: 7,
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    assert_eq!(
        json,
        r#"{"type":"session/subscribed","sessionId":"s1","lastSeq":7}"#
    );
    let back: MuxFrame = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, frame);

    let frame = MuxFrame::QuestionResolved {
        session_id: session_id("s1"),
        question_rpc_id: rpc_id("q1"),
        outcome: QuestionOutcome::Answered,
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["type"], "question/resolved");
    assert_eq!(parsed["questionRpcId"], "q1");
    assert_eq!(parsed["outcome"], "answered");
    let back: MuxFrame = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, frame);

    let frame = MuxFrame::StreamError {
        error: RpcError::Internal(RpcErrorBody {
            message: "m".to_string(),
            details: EmptyDetails {},
        }),
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    assert!(json.contains(r#""type":"stream/error""#), "{json}");
    let back: MuxFrame = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, frame);

    // Unknown frame type fails loud.
    assert!(serde_json::from_str::<MuxFrame>(r#"{"type":"nope"}"#).is_err());
}

#[test]
fn session_event_frames_carry_the_domain_event_serde_form() {
    let event = SessionEvent {
        type_: "turn/start".to_string(),
        seq: 3,
        time: 1700000000000,
        data: serde_json::json!({"turn": 1}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    let frame = MuxFrame::SessionEventFrame {
        session_id: session_id("s1"),
        event: event.clone(),
        view: None,
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["type"], "session/event");
    assert_eq!(parsed["event"]["type"], "turn/start");
    assert_eq!(parsed["event"]["seq"], 3);
    assert_eq!(parsed["event"]["data"]["turn"], 1);
    // `view` stays absent, matching the TS optional field.
    assert!(parsed.get("view").is_none());
    let back: MuxFrame = serde_json::from_str(&json).expect("reparse");
    match back {
        MuxFrame::SessionEventFrame { event, .. } => assert_eq!(event, event),
        other => panic!("expected session/event frame, got {other:?}"),
    }
}

#[test]
fn host_frames_discriminate_and_roundtrip() {
    let frame = HostFrame::SessionAdded {
        session_id: session_id("s1"),
        blank: true,
        parent_session_id: None,
        origin: None,
        cwd: Some("/proj".to_string()),
        agent_preset: None,
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    assert_eq!(
        json,
        r#"{"type":"host/session-added","sessionId":"s1","blank":true,"cwd":"/proj"}"#
    );
    let back: HostFrame = serde_json::from_str(&json).expect("reparse");
    assert_eq!(back, frame);

    let frame = HostFrame::RemoteEvent {
        event: "custom".to_string(),
        args: vec![serde_json::json!({"a": 1})],
    };
    let json = serde_json::to_string(&frame).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["type"], "host/remote-event");
    assert_eq!(parsed["args"][0]["a"], 1);

    assert!(serde_json::from_str::<HostFrame>(r#"{"type":"nope"}"#).is_err());
}

#[test]
fn queued_inbox_item_wire_shape() {
    let item = QueuedInboxItem {
        id: message_id("m1"),
        placement: QueuedInboxPlacement::Queued,
        message: Message {
            id: message_id("m1"),
            role: Role::User,
            content: vec![dsh_llm::ContentBlock::Text {
                text: "hello".to_string(),
            }],
            source: MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        },
    };
    let json = serde_json::to_string(&item).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(parsed["id"], "m1");
    assert_eq!(parsed["placement"], "queued");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"][0]["text"], "hello");
    assert_eq!(parsed["message"]["source"]["kind"], "user");
}
