//! Rust port of the core `compaction.spec.ts` + `tool-pairing.spec.ts` +
//! `invariant.spec.ts` behaviors: the checkpoint source predicate, the
//! tool-pairing balance fold, the result vocabulary, and the compaction
//! bracket state machine.

use dsh_compaction::{
    CompactionTrigger, ManualCompactionError, ManualCompactionErrorCode, compact_checkpoint_source,
    compaction_id, invariant::apply_compaction_event, is_compact_checkpoint_source,
    tool_pairing_balanced_after, tool_pairing_balanced_before,
};
use dsh_llm::{MessageSource, create_user_message};
use dsh_session::{Session, SurfaceIntent, SurfaceOp, session_id};
use dsh_compaction::invariant::SessionTrace;

fn message(session: &Session, source: MessageSource, text: &str) -> dsh_session::SessionEvent {
    let message = create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: text.to_string(),
        }],
        source,
    );
    session
        .append(
            "user/message",
            serde_json::to_value(&message).expect("message"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("user/message")
}

fn assistant_with_tool_calls(session: &Session, count: usize) -> dsh_session::SessionEvent {
    let blocks: Vec<dsh_llm::ContentBlock> = (0..count)
        .map(|index| dsh_llm::ContentBlock::ToolCall {
            id: dsh_llm::call_id(&format!("c{index}")),
            name: "probe".to_string(),
            arguments: "{}".to_string(),
        })
        .collect();
    let message = dsh_llm::create_assistant_message(
        blocks,
        dsh_llm::ModelMessageSource {
            provider: "stub".to_string(),
            model: "stub".to_string(),
            replay_state: None,
        },
    );
    session
        .append(
            "assistant/message",
            dsh_session::assistant_message_data(1, 1, &message, None),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("assistant/message")
}

fn tool_result(session: &Session, call_id: &str) -> dsh_session::SessionEvent {
    let message = dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
        call_id: dsh_llm::call_id(call_id),
        content: vec![dsh_llm::ContentBlock::Text {
            text: "ok".to_string(),
        }],
        is_error: false,
    });
    session
        .append(
            "tool/result",
            serde_json::to_value(&message).expect("message"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("tool/result")
}

#[test]
fn checkpoint_sources_carry_the_compact_marker() {
    let id = compaction_id("c-1");
    let source = compact_checkpoint_source(&id, None);
    assert!(is_compact_checkpoint_source(&source));
    let MessageSource::Plugin { plugin, compaction_id: carried, source_command_id, .. } = &source
    else {
        panic!("plugin source");
    };
    assert_eq!(plugin, "compact");
    assert_eq!(carried.as_deref(), Some("c-1"));
    assert_eq!(source_command_id, &None);

    let with_command = compact_checkpoint_source(&id, Some(&dsh_commands::command_id("cmd-1")));
    let MessageSource::Plugin { source_command_id, .. } = &with_command else {
        panic!("plugin source");
    };
    assert_eq!(source_command_id.as_deref(), Some("cmd-1"));

    let ordinary = MessageSource::User {
        rpc_id: None,
        client_time_zone: None,
    };
    assert!(!is_compact_checkpoint_source(&ordinary));
}

#[test]
fn tool_pairing_balances_cuts_around_paired_calls() {
    let session = Session::create(session_id("pairing"), None, None).expect("session");
    let before_any = tool_pairing_balanced_before(&session, 0);
    assert!(before_any.is_err(), "no surface node yet");

    message(&session, MessageSource::User { rpc_id: None, client_time_zone: None }, "go");
    let with_calls = assistant_with_tool_calls(&session, 2);
    let result = tool_result(&session, "c0");
    let _ = result;
    let _ = with_calls;
    // After one result, one call remains open: the cut after the tool/result
    // is unbalanced, the cut before the assistant message is balanced.
    let surface = session.surface().expect("surface");
    assert_eq!(surface.nodes.len(), 3);
    let assistant_seq = 1;
    assert!(
        tool_pairing_balanced_before(&session, assistant_seq).expect("before assistant"),
        "no open call before the assistant message"
    );
    assert!(
        !tool_pairing_balanced_after(&session, 2).expect("after result"),
        "one call still open after the first result"
    );
    let second = tool_result(&session, "c1");
    let _ = second;
    assert!(tool_pairing_balanced_after(&session, 3).expect("after second result"));
}

#[test]
fn result_and_error_vocabulary() {
    assert_eq!(CompactionTrigger::Pressure.as_str(), "pressure");
    assert_eq!(
        CompactionTrigger::ContextOverflow.as_str(),
        "context-overflow"
    );
    let error = ManualCompactionError::new(
        ManualCompactionErrorCode::Busy,
        "another compaction is running",
    );
    assert_eq!(error.code.as_str(), "busy");
    assert_eq!(error.to_string(), "another compaction is running");
    let result = dsh_compaction::CompactionResult {
        compaction_id: compaction_id("c-1"),
        source_command_id: None,
        start_seq: 1,
        summary_seq: 3,
        end_seq: 5,
        summary: Vec::new(),
        shadowed_range: (2, 4),
        shadowed_seqs: vec![2, 3, 4],
        shadowed_token_count: 10,
    };
    assert_eq!(result.shadowed_seqs, vec![2, 3, 4]);
}

#[test]
fn compaction_trace_rejects_malformed_brackets() {
    let session = Session::create(session_id("trace"), None, None).expect("session");
    let mut trace = SessionTrace::default();
    let start = session
        .append(
            "compaction/start",
            serde_json::json!({ "compactionId": "c-1", "turn": null }),
            None,
        )
        .expect("start");
    apply_compaction_event(&mut trace, &start).expect("start ok");
    let overlapping = session
        .append(
            "compaction/start",
            serde_json::json!({ "compactionId": "c-2", "turn": null }),
            None,
        )
        .expect("second start");
    assert!(apply_compaction_event(&mut trace, &overlapping)
        .unwrap_err()
        .contains("overlaps an open compaction"));

    let summary = session
        .append(
            "compaction/summary",
            serde_json::json!({ "compactionId": "c-1", "summary": [], "shadowedRange": {"start": 1, "end": 1}, "shadowedSeqs": [], "shadowedTokenCount": 0, "provider": "p", "model": "m" }),
            None,
        )
        .expect("summary");
    apply_compaction_event(&mut trace, &summary).expect("summary ok");
    let end = session
        .append(
            "compaction/end",
            serde_json::json!({ "compactionId": "c-1", "turn": null }),
            None,
        )
        .expect("end");
    apply_compaction_event(&mut trace, &end).expect("end ok");
    assert!(trace.compaction.is_none());

    // A checkpoint outside any open compaction fails.
    let checkpoint = message(
        &session,
        compact_checkpoint_source(&compaction_id("c-9"), None),
        "summary text",
    );
    assert!(apply_compaction_event(&mut trace, &checkpoint)
        .unwrap_err()
        .contains("no matching compaction/start"));

    // A turn boundary crossing an open standalone compaction fails.
    let mut trace = SessionTrace::default();
    let start = session
        .append(
            "compaction/start",
            serde_json::json!({ "compactionId": "c-3", "turn": null }),
            None,
        )
        .expect("start");
    apply_compaction_event(&mut trace, &start).expect("start ok");
    let turn = session
        .append("turn/start", serde_json::json!({ "turn": 2 }), None)
        .expect("turn/start");
    assert!(apply_compaction_event(&mut trace, &turn)
        .unwrap_err()
        .contains("cannot cross an open standalone compaction"));
}
