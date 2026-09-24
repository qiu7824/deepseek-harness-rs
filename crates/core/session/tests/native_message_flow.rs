use dsh_llm::{ContentBlock, Role, ToolResultMessageInput, call_id, create_tool_result_message};
use dsh_session::{
    Session, SessionLogOffset, SurfaceIntent, SurfaceOp, session_id, tool_result_data,
};

fn append() -> Option<SurfaceIntent> {
    Some(SurfaceIntent {
        surface_op: SurfaceOp::Append,
        source_event_seqs: None,
    })
}

#[test]
fn flat_tool_result_survives_append_restore_and_content_pruning() {
    let session = Session::create(session_id("native-result"), None, None, None).unwrap();
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id: call_id("call-1"),
        content: vec![ContentBlock::Text {
            text: "original result".into(),
        }],
        is_error: true,
    });
    let original = session
        .append(
            "tool/result",
            tool_result_data(1, 1, &message, None, None),
            append(),
        )
        .unwrap();
    let restored = Session::from_restore(
        session.id().clone(),
        session.events().as_ref().clone(),
        session.header(),
        SessionLogOffset::ZERO,
    )
    .unwrap();
    assert_eq!(
        restored.derive_messages().unwrap().as_ref(),
        &vec![message.clone()]
    );
    let mut pruned = message.clone();
    pruned.content = vec![];
    restored
        .append(
            "tool/result",
            tool_result_data(1, 1, &pruned, None, None),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace {
                    start: original.seq.get(),
                    end: original.seq.get(),
                },
                source_event_seqs: Some(vec![original.seq.get()]),
            }),
        )
        .unwrap();
    let replay = restored.derive_messages().unwrap();
    assert_eq!(replay[0].role, Role::Tool);
    assert_eq!(
        replay[0].as_tool_result().unwrap(),
        (&call_id("call-1"), &[][..], Some(true))
    );
    assert_eq!(replay[0].id, message.id);
}

#[test]
fn malformed_tool_correlation_is_rejected_before_publication_and_on_restore() {
    let session = Session::create(session_id("invalid-result"), None, None, None).unwrap();
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id: call_id("call-1"),
        content: vec![],
        is_error: false,
    });
    let mut data = tool_result_data(1, 1, &message, None, None);
    data["message"]["toolCallId"] = "different-call".into();
    assert!(
        session
            .append("tool/result", data.clone(), append())
            .unwrap_err()
            .contains("mismatched")
    );
    assert_eq!(session.seq(), SessionLogOffset::ZERO);
    let event = serde_json::from_value(
        serde_json::json!({"type":"tool/result","seq":0,"time":0,"data":data,"surfaceOp":"append"}),
    )
    .unwrap();
    assert!(
        Session::from_restore(
            session.id().clone(),
            vec![event],
            session.header(),
            SessionLogOffset::ZERO
        )
        .unwrap_err()
        .contains("mismatched")
    );
}
