use super::*;
use dsh_llm::BlockAssembler;

fn message(id: &str, phase: Option<&str>, parts: &[&str]) -> Value {
    let mut value = json!({"id":id,"type":"message","role":"assistant","status":"completed","content":parts.iter().map(|text|json!({"type":"output_text","text":text})).collect::<Vec<_>>()});
    if let Some(phase) = phase {
        value["phase"] = json!(phase);
    }
    value
}
fn collect(events: Vec<Value>) -> Vec<StreamChunk> {
    let mut parser = ResponsesTranslator::default();
    let mut chunks = Vec::new();
    for event in events {
        chunks.extend(parser.consume(&event.to_string()).unwrap());
    }
    chunks
}
fn assembler(chunks: &[StreamChunk]) -> BlockAssembler {
    let mut value = BlockAssembler::new();
    for chunk in chunks {
        value.push(chunk);
    }
    value
}
fn text(chunks: &[StreamChunk]) -> String {
    assembler(chunks)
        .interrupted_blocks()
        .into_iter()
        .filter_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text)
            } else {
                None
            }
        })
        .collect()
}
fn state(chunks: &[StreamChunk]) -> &Value {
    chunks
        .iter()
        .rev()
        .find_map(|chunk| {
            if let StreamChunk::Finish {
                replay_state: Some(state),
                ..
            } = chunk
            {
                Some(state)
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn done_items_and_all_content_parts_survive_a_sparse_terminal_snapshot() {
    let first = message(
        "m0",
        Some("commentary"),
        &["First paragraph.\n\n", "Second paragraph.\n\n"],
    );
    let last = message("m1", Some("final_answer"), &["Final answer."]);
    let chunks = collect(vec![
        json!({"type":"response.output_item.done","output_index":1,"item":last}),
        json!({"type":"response.output_item.done","output_index":0,"item":first}),
        json!({"type":"response.output_item.done","output_index":0,"item":first}),
        json!({"type":"response.completed","response":{"output":[message("m1",Some("final_answer"),&[""])]}}),
    ]);
    assert_eq!(
        text(&chunks),
        "First paragraph.\n\nSecond paragraph.\n\nFinal answer."
    );
    assert_eq!(state(&chunks)["items"].as_array().unwrap().len(), 2);
    assert_eq!(state(&chunks)["items"][0]["id"], "m0");
    assert_eq!(state(&chunks)["items"][1]["id"], "m1");
    assert_eq!(state(&chunks)["hasVisibleFinal"], true);
    assert_eq!(assembler(&chunks).finish(), FinishReason::Stop);
}

#[test]
fn a_terminal_part_can_extend_done_text_but_cannot_replace_a_longer_confirmed_content_array() {
    let chunks = collect(vec![
        json!({"type":"response.output_text.delta","item_id":"m0","output_index":0,"content_index":0,"delta":"First"}),
        json!({"type":"response.output_item.done","output_index":0,"item":message("m0",None,&["First"," second"])}),
        json!({"type":"response.completed","response":{"output":[message("m0",None,&[" second"])]}}),
    ]);
    assert_eq!(text(&chunks), "First second");
    let chunks = collect(vec![
        json!({"type":"response.output_item.done","item":message("m0",None,&["First"])}),
        json!({"type":"response.completed","response":{"output":[message("m0",None,&["First complete"])]}}),
    ]);
    assert_eq!(text(&chunks), "First complete");
}

#[test]
fn token_exhaustion_retains_text_and_usage_but_never_finalizes_or_replays_tools() {
    for with_tool in [false, true] {
        let mut events = vec![
            json!({"type":"response.output_text.delta","item_id":"m0","output_index":0,"content_index":0,"delta":"Saved prefix"}),
        ];
        if with_tool {
            events.push(json!({"type":"response.output_item.added","output_index":1,"item":{"id":"f0","type":"function_call","call_id":"call0","name":"write","arguments":"","status":"in_progress"}}));
            events.push(json!({"type":"response.function_call_arguments.delta","item_id":"f0","delta":"{\"path\":"}));
        }
        events.push(json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[message("m0",None,&["Saved prefix plus terminal text"])],"usage":{"input_tokens":25,"output_tokens":16}}}));
        let chunks = collect(events);
        let assembled = assembler(&chunks);
        assert_eq!(assembled.finish(), FinishReason::MaxTokens);
        assert_eq!(text(&chunks), "Saved prefix plus terminal text");
        assert_eq!(assembled.usage().unwrap().output_tokens, 16);
        assert_eq!(state(&chunks)["responseStatus"], "incomplete");
        assert_eq!(state(&chunks)["incompleteReason"], "max_output_tokens");
        assert_eq!(state(&chunks)["truncatedToolCalls"], with_tool);
        assert_eq!(state(&chunks)["items"], json!([]));
        assert!(!chunks.iter().any(|chunk| matches!(
            chunk,
            StreamChunk::BlockEnd {
                block: ContentBlock::ToolCall { .. },
                ..
            }
        )));
        assert!(
            !assembled
                .blocks()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall { .. }))
        );
    }
}

#[test]
fn unknown_incomplete_and_content_filter_are_explicit_non_success_outcomes() {
    for (reason, code) in [
        ("server_pause", "INCOMPLETE_RESPONSE"),
        ("content_filter", "CONTENT_FILTER"),
    ] {
        let chunks = collect(vec![
            json!({"type":"response.output_item.done","item":message("m0",None,&["Received text"])}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":reason},"usage":{"input_tokens":3,"output_tokens":4}}}),
        ]);
        assert!(
            matches!(assembler(&chunks).finish(),FinishReason::Error{failure}if failure.code==code)
        );
        assert_eq!(text(&chunks), "Received text");
        assert_eq!(state(&chunks)["incompleteReason"], reason);
        assert_eq!(state(&chunks)["responseStatus"], "incomplete");
    }
}

#[test]
fn empty_completed_and_empty_final_after_commentary_are_identified_without_erasing_the_preamble() {
    let empty = collect(vec![
        json!({"type":"response.completed","response":{"output":[]}}),
    ]);
    assert_eq!(state(&empty)["hasVisibleFinal"], false);
    assert_eq!(state(&empty)["responseStatus"], "completed");
    let chunks = collect(vec![
        json!({"type":"response.output_item.done","output_index":0,"item":message("m0",Some("commentary"),&["Checking now."])}),
        json!({"type":"response.completed","response":{"output":[message("m1",Some("final_answer"),&[""])]}}),
    ]);
    assert_eq!(text(&chunks), "Checking now.");
    assert_eq!(state(&chunks)["hasVisibleFinal"], false);
}

#[test]
fn only_confirmed_valid_function_arguments_may_be_dispatched() {
    let added = json!({"type":"response.output_item.added","output_index":0,"item":{"id":"f0","type":"function_call","call_id":"c0","name":"read","arguments":"","status":"in_progress"}});
    let completed = json!({"type":"response.completed","response":{"output":[]}});
    let unconfirmed = collect(vec![
        added.clone(),
        json!({"type":"response.function_call_arguments.delta","item_id":"f0","delta":"{}"}),
        completed.clone(),
    ]);
    assert!(
        matches!(assembler(&unconfirmed).finish(),FinishReason::Error{failure}if failure.code=="INCOMPLETE_TOOL_CALL")
    );
    for args in ["{}", ""] {
        let confirmed = collect(vec![
            added.clone(),
            json!({"type":"response.function_call_arguments.done","item_id":"f0","arguments":args}),
            completed.clone(),
        ]);
        assert_eq!(assembler(&confirmed).finish(), FinishReason::ToolCalls);
        assert!(
            assembler(&confirmed).blocks().iter().any(
                |block| matches!(block,ContentBlock::ToolCall{arguments,..}if arguments=="{}")
            )
        );
    }
    let invalid = collect(vec![
        added,
        json!({"type":"response.function_call_arguments.done","item_id":"f0","arguments":"{\"path\":"}),
        completed,
    ]);
    assert!(
        matches!(assembler(&invalid).finish(),FinishReason::Error{failure}if failure.code=="INCOMPLETE_TOOL_CALL")
    );
}

#[test]
fn streamed_refusal_is_not_converted_to_an_empty_retryable_success() {
    let chunks = collect(vec![
        json!({"type":"response.refusal.delta","delta":"Cannot comply."}),
        json!({"type":"response.completed","response":{"output":[]}}),
    ]);
    assert_eq!(text(&chunks), "Cannot comply.");
    assert!(
        matches!(assembler(&chunks).finish(),FinishReason::Error{failure}if failure.code=="CONTENT_FILTER")
    );
}

#[test]
fn done_text_recovers_missing_deltas_and_ordinary_text_has_a_visible_final() {
    for identified in [false, true] {
        let mut event = json!({"type":"response.output_text.done","text":"Complete text"});
        if identified {
            event["item_id"] = json!("m0");
            event["output_index"] = json!(0);
        }
        let chunks = collect(vec![
            event,
            json!({"type":"response.completed","response":{"output":[]}}),
        ]);
        assert_eq!(text(&chunks), "Complete text");
        assert_eq!(state(&chunks)["hasVisibleFinal"], true);
        assert_eq!(assembler(&chunks).finish(), FinishReason::Stop);
    }
    let chunks = collect(vec![
        json!({"type":"response.completed","response":{"output":[message("m0",None,&["Normal answer"])]}}),
    ]);
    assert_eq!(state(&chunks)["hasVisibleFinal"], true);
    assert!(state(&chunks).get("continuation").is_none());
}

#[test]
fn refusal_done_and_terminal_refusal_never_allow_empty_answer_recovery() {
    for refusal in [
        json!({"type":"response.refusal.done","item_id":"m1","refusal":"Cannot comply."}),
        json!({"type":"response.output_item.done","item":{"id":"m1","type":"message","role":"assistant","content":[{"type":"refusal","refusal":"Cannot comply."}]}}),
        json!({"type":"response.completed","response":{"output":[{"id":"m1","type":"message","role":"assistant","content":[{"type":"refusal","refusal":"Cannot comply."}]}]}}),
    ] {
        let chunks = collect(vec![
            json!({"type":"response.output_item.done","output_index":0,"item":message("m0",Some("commentary"),&["Checking. "])}),
            refusal,
            json!({"type":"response.completed","response":{"output":[]}}),
        ]);
        assert_eq!(text(&chunks), "Checking. Cannot comply.");
        assert!(
            matches!(assembler(&chunks).finish(),FinishReason::Error{failure}if failure.code=="CONTENT_FILTER")
        );
        assert!(state(&chunks).get("continuation").is_none());
    }
    let chunks = collect(vec![
        json!({"type":"response.refusal.delta","item_id":"m0","delta":"Cannot"}),
        json!({"type":"response.refusal.done","item_id":"m0","refusal":"Cannot comply."}),
        json!({"type":"response.completed","response":{"output":[]}}),
    ]);
    assert_eq!(text(&chunks), "Cannot comply.");
}

#[test]
fn transport_failure_after_done_only_text_preserves_the_received_prefix_once() {
    let mut parser = ResponsesTranslator::default();
    let mut chunks=parser.consume(&json!({"type":"response.output_item.done","item":message("m0",None,&["Coalesced text"])}).to_string()).unwrap();
    chunks.extend(parser.fail(failure("connection lost", "TRANSPORT")));
    assert_eq!(text(&chunks), "Coalesced text");
    assert_eq!(state(&chunks)["responseStatus"], "failed");
    assert!(
        matches!(assembler(&chunks).finish(),FinishReason::Error{failure}if failure.code=="TRANSPORT")
    );
    assert!(parser.fail(failure("duplicate", "TRANSPORT")).is_empty());
}

#[test]
fn terminal_chunk_limit_still_returns_an_error_and_safe_text_without_tool_execution() {
    let mut parser = ResponsesTranslator::default();
    let chunks = parser
        .consume_limited(
            &json!({"type":"response.completed","response":{"output":[
                message("m0",None,&["Received text"]),
                {"type":"function_call","id":"f0","call_id":"c0","name":"write","arguments":"{}"}
            ]}})
            .to_string(),
            1,
        )
        .unwrap();
    assert_eq!(text(&chunks), "Received text");
    assert!(
        matches!(assembler(&chunks).finish(), FinishReason::Error{failure} if failure.code=="RESPONSE_TOO_LARGE")
    );
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
            .count(),
        1
    );
    assert!(!chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::BlockEnd {
            block: ContentBlock::ToolCall { .. },
            ..
        }
    )));
    assert_eq!(state(&chunks)["items"], json!([]));
}
