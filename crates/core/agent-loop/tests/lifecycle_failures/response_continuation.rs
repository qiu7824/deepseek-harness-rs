use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use dsh_agent::{Agent, AgentCancelCause, AgentStatus};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmFailure, StreamChunk,
    call_id,
};
use serde_json::json;

use super::support::{harness, message, quick_tool, register_adapter, turn_end_kinds};

#[derive(Clone, Copy)]
enum Reply {
    Commentary(&'static str),
    Final(&'static str),
    Plain(&'static str),
    Empty,
    Tool,
    DroppedTool,
    Limit(&'static str),
    TruncatedTool,
    TruncatedToolMetadata,
    ReasoningLimit,
    Failure(&'static str, &'static str),
    Aborted(&'static str),
    EmptyFinalAfterCommentary,
    UnphasedWithFalseFinalHint,
}

struct Adapter {
    replies: Vec<Reply>,
    calls: AtomicUsize,
    histories: Mutex<Vec<Vec<String>>>,
    output_budgets: Mutex<Vec<Option<u64>>>,
    pending_on: Option<usize>,
    entered: tokio::sync::Notify,
}
impl Adapter {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies,
            calls: AtomicUsize::new(0),
            histories: Mutex::new(Vec::new()),
            output_budgets: Mutex::new(Vec::new()),
            pending_on: None,
            entered: tokio::sync::Notify::new(),
        }
    }
}
impl LlmAdapter for Adapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.output_budgets.lock().unwrap().push(options.max_tokens);
        self.histories.lock().unwrap().push(
            options
                .messages
                .iter()
                .flat_map(|message| message.content.iter())
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect(),
        );
        if self.pending_on == Some(index) {
            self.entered.notify_one();
            return Box::pin(futures::stream::pending());
        }
        let reply = self.replies[index.min(self.replies.len() - 1)];
        if matches!(reply, Reply::Tool | Reply::TruncatedTool) {
            return Box::pin(futures::stream::iter(vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".into(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: call_id(format!("call-{index}")),
                        name: "quick".into(),
                        arguments: if matches!(reply, Reply::TruncatedTool) {
                            "{"
                        } else {
                            "{}"
                        }
                        .into(),
                    },
                },
                StreamChunk::Finish {
                    reason: if matches!(reply, Reply::TruncatedTool) {
                        FinishReason::MaxTokens
                    } else {
                        FinishReason::ToolCalls
                    },
                    replay_state: None,
                },
            ]));
        }
        if let Reply::Failure(text, _) | Reply::Aborted(text) = reply {
            let code = match reply {
                Reply::Failure(_, code) => code,
                _ => "ABORTED",
            };
            let failure = LlmFailure {
                code: code.into(),
                message: "provider stopped".into(),
                status: None,
                provider_retry_after_ms: None,
                request_id: None,
            };
            return Box::pin(futures::stream::iter(vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".into(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: text.into(),
                },
                StreamChunk::BlockStart {
                    index: 1,
                    block_type: "tool-call".into(),
                },
                StreamChunk::ToolCallDelta {
                    index: 1,
                    id: call_id("broken"),
                    name: Some("quick".into()),
                    arguments_delta: "{".into(),
                },
                StreamChunk::Finish {
                    reason: if matches!(reply, Reply::Aborted(_)) {
                        FinishReason::Aborted { failure }
                    } else {
                        FinishReason::Error { failure }
                    },
                    replay_state: None,
                },
            ]));
        }
        if matches!(reply, Reply::ReasoningLimit) {
            return Box::pin(futures::stream::iter(vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "reasoning".into(),
                },
                StreamChunk::ReasoningDelta {
                    index: 0,
                    text: "still reasoning".into(),
                },
                StreamChunk::Finish {
                    reason: FinishReason::MaxTokens,
                    replay_state: None,
                },
            ]));
        }
        let (text, phase) = match reply {
            Reply::Commentary(text) => (text, Some("commentary")),
            Reply::Final(text) => (text, Some("final_answer")),
            Reply::Plain(text) => (text, None),
            Reply::Empty => ("", None),
            Reply::DroppedTool => ("I will call the tool.", None),
            Reply::Limit(text) => (text, None),
            Reply::TruncatedToolMetadata => ("Visible prefix before tool", None),
            Reply::EmptyFinalAfterCommentary => ("Checking the file.", Some("commentary")),
            Reply::UnphasedWithFalseFinalHint => ("A normal unphased answer.", None),
            _ => unreachable!(),
        };
        let mut state = json!({"format":"openai-responses-v1","items":[{"type":"message","role":"assistant","phase":phase,"content":[{"type":"output_text","text":text}]}]});
        if phase == Some("commentary") {
            state["continuation"] = json!("commentary");
        }
        if matches!(reply, Reply::EmptyFinalAfterCommentary) {
            state.as_object_mut().unwrap().remove("continuation");
            state["hasVisibleFinal"] = json!(false);
            state["items"].as_array_mut().unwrap().push(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":[]}),
            );
        }
        if matches!(reply, Reply::UnphasedWithFalseFinalHint) {
            state["hasVisibleFinal"] = json!(false);
        }
        if matches!(reply, Reply::TruncatedToolMetadata) {
            state["truncatedToolCalls"] = json!(true);
        }
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text { text: text.into() },
            },
            StreamChunk::Finish {
                reason: match reply {
                    Reply::DroppedTool => FinishReason::ToolCalls,
                    Reply::Limit(_) | Reply::TruncatedToolMetadata => FinishReason::MaxTokens,
                    _ => FinishReason::Stop,
                },
                replay_state: Some(state),
            },
        ]))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commentary_continues_in_the_same_turn_and_keeps_its_history() {
    let harness = harness().await;
    let adapter = Arc::new(Adapter::new(vec![
        Reply::Commentary("Done."),
        Reply::Final("I will explain the verified result."),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(
        adapter.calls.load(Ordering::SeqCst),
        2,
        "phase controls continuation, not text keywords"
    );
    assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
    assert!(
        adapter.histories.lock().unwrap()[1]
            .iter()
            .any(|text| text == "Done.")
    );
    assert_eq!(
        harness
            .agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "assistant/message")
            .count(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_commentary_or_empty_followups_stop_with_an_incomplete_error() {
    for replies in [
        vec![Reply::Commentary("Checking.")],
        vec![Reply::Commentary("Checking."), Reply::Empty],
    ] {
        let harness = harness().await;
        let adapter = Arc::new(Adapter::new(replies));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 4);
        assert_eq!(turn_end_kinds(&harness.agent), ["error"]);
        assert!(
            harness
                .agent
                .session()
                .events()
                .iter()
                .any(|event| event.type_ == "turn/end"
                    && event.data["reason"]["error"]["code"] == "INCOMPLETE_RESPONSE")
        );
        assert_eq!(harness.agent.status(), AgentStatus::Idle);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_nonempty_final_and_unphased_responses_do_not_gain_retries() {
    for reply in [Reply::Final("Checking."), Reply::Plain("Checking.")] {
        let harness = harness().await;
        let adapter = Arc::new(Adapter::new(vec![reply]));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_tool_progress_resets_the_intermediate_response_budget() {
    let harness = harness().await;
    let executed = Arc::new(AtomicBool::new(false));
    harness
        .tools
        .register(&harness.ctx, quick_tool(executed.clone()))
        .unwrap();
    let adapter = Arc::new(Adapter::new(vec![
        Reply::Commentary("one"),
        Reply::Commentary("two"),
        Reply::Commentary("three"),
        Reply::Tool,
        Reply::Commentary("four"),
        Reply::Commentary("five"),
        Reply::Commentary("six"),
        Reply::Final("result"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert!(executed.load(Ordering::SeqCst));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 8);
    assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_continuation_does_not_start_another_model_call() {
    let harness = harness().await;
    let mut adapter = Adapter::new(vec![Reply::Commentary("checking")]);
    adapter.pending_on = Some(1);
    let adapter = Arc::new(adapter);
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), adapter.entered.notified())
        .await
        .unwrap();
    harness.agent.cancel(AgentCancelCause::User, None);
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(turn_end_kinds(&harness.agent), ["aborted"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_and_post_tool_empty_answers_recover_before_completing() {
    for replies in [
        vec![Reply::Empty, Reply::Final("result")],
        vec![Reply::Tool, Reply::Empty, Reply::Final("result")],
    ] {
        let harness = harness().await;
        harness
            .tools
            .register(&harness.ctx, quick_tool(Arc::new(AtomicBool::new(false))))
            .unwrap();
        let expected = replies.len();
        let adapter = Arc::new(Adapter::new(replies));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), expected);
        assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
        assert!(
            harness
                .agent
                .session()
                .events()
                .iter()
                .any(|event| event.type_ == "user/message"
                    && event.data["source"]["plugin"] == "agent-loop:response-recovery")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_and_dropped_tool_responses_have_a_finite_recovery_budget() {
    for reply in [Reply::Empty, Reply::DroppedTool] {
        let harness = harness().await;
        let adapter = Arc::new(Adapter::new(vec![reply]));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
        assert_eq!(turn_end_kinds(&harness.agent), ["error"]);
        assert!(
            harness
                .agent
                .session()
                .events()
                .iter()
                .any(|event| event.type_ == "turn/end"
                    && event.data["reason"]["error"]["code"] == "INCOMPLETE_RESPONSE")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovered_tool_call_is_executed_once() {
    let harness = harness().await;
    let executed = Arc::new(AtomicBool::new(false));
    harness
        .tools
        .register(&harness.ctx, quick_tool(executed.clone()))
        .unwrap();
    let adapter = Arc::new(Adapter::new(vec![
        Reply::DroppedTool,
        Reply::Tool,
        Reply::Final("result"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert!(executed.load(Ordering::SeqCst));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        harness
            .agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "tool/call")
            .count(),
        1
    );
    assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_truncation_preserves_each_part_without_raising_output_budgets() {
    let harness = harness().await;
    let adapter = Arc::new(Adapter::new(vec![
        Reply::Limit("first part"),
        Reply::Limit("second part"),
        Reply::Final("last part"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
    assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
    let histories = adapter.histories.lock().unwrap();
    assert!(histories[2].iter().any(|text| text == "first part"));
    assert!(histories[2].iter().any(|text| text == "second part"));
    let budgets = adapter.output_budgets.lock().unwrap();
    assert!(budgets.iter().all(|budget| *budget == budgets[0]));
    let parts = harness
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "assistant/message")
        .map(|event| {
            event.data["message"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(parts, ["first part", "second part", "last part"]);
    let events = harness.agent.session().events();
    let saved = events
        .iter()
        .filter(|event| event.type_ == "assistant/message")
        .collect::<Vec<_>>();
    assert_eq!(saved[0].data["truncated"], true);
    assert_eq!(saved[1].data["truncated"], true);
    assert!(saved[2].data.get("truncated").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_progress_cannot_refill_the_turns_truncation_budget() {
    let harness = harness().await;
    harness
        .tools
        .register(&harness.ctx, quick_tool(Arc::new(AtomicBool::new(false))))
        .unwrap();
    let adapter = Arc::new(Adapter::new(vec![
        Reply::Limit("one"),
        Reply::Tool,
        Reply::Limit("two"),
        Reply::Limit("three"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 4);
    assert_eq!(turn_end_kinds(&harness.agent), ["error"]);
    assert!(
        harness
            .agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "turn/end"
                && event.data["reason"]["error"]["code"] == "OUTPUT_CONTINUATION_LIMIT")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_tools_and_reasoning_only_limits_are_not_retried_or_executed() {
    for (reply, code) in [
        (Reply::TruncatedTool, "TRUNCATED_TOOL_CALL"),
        (Reply::TruncatedToolMetadata, "TRUNCATED_TOOL_CALL"),
        (Reply::ReasoningLimit, "OUTPUT_LIMIT_WITHOUT_ANSWER"),
    ] {
        let harness = harness().await;
        let executed = Arc::new(AtomicBool::new(false));
        harness
            .tools
            .register(&harness.ctx, quick_tool(executed.clone()))
            .unwrap();
        let adapter = Arc::new(Adapter::new(vec![reply]));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert!(!executed.load(Ordering::SeqCst));
        assert!(harness.agent.session().events().iter().any(
            |event| event.type_ == "turn/end" && event.data["reason"]["error"]["code"] == code
        ));
        for event in harness
            .agent
            .session()
            .events()
            .iter()
            .filter(|event| event.type_ == "assistant/message")
        {
            assert!(
                event.data["message"]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|block| block["type"] != "tool-call")
            );
            assert!(event.data["message"]["source"].get("replayState").is_none());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_transport_errors_and_refusals_keep_the_safe_prefix_without_recovery() {
    for (prefix, code) in [
        ("received prefix", "TRANSPORT"),
        ("refused", "CONTENT_FILTER"),
    ] {
        let harness = harness().await;
        let adapter = Arc::new(Adapter::new(vec![Reply::Failure(prefix, code)]));
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(turn_end_kinds(&harness.agent), ["error"]);
        let events = harness.agent.session().events();
        let saved = events
            .iter()
            .filter(|event| event.type_ == "assistant/message")
            .collect::<Vec<_>>();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].data["message"]["content"][0]["text"], prefix);
        assert_eq!(saved[0].data["interrupted"], true);
        assert_eq!(
            saved[0].data["message"]["content"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(!events.iter().any(|event| event.type_ == "tool/call"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_cannot_continue_after_a_safety_refusal() {
    let harness = harness().await;
    let adapter = Arc::new(Adapter::new(vec![
        Reply::Limit("partial answer"),
        Reply::Failure("refused", "CONTENT_FILTER"),
        Reply::Final("must not run"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(turn_end_kinds(&harness.agent), ["error"]);
    assert!(
        harness
            .agent
            .session()
            .events()
            .iter()
            .any(|event| event.type_ == "turn/end"
                && event.data["reason"]["error"]["code"] == "CONTENT_FILTER")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_listeners_cannot_override_provider_cancellation_or_safety_refusal() {
    for (reply, code) in [
        (
            Reply::Failure("safe prefix", "CONTENT_FILTER"),
            "CONTENT_FILTER",
        ),
        (Reply::Aborted("safe prefix"), "ABORTED"),
    ] {
        let harness = harness().await;
        let adapter = Arc::new(Adapter::new(vec![reply, Reply::Final("must not run")]));
        register_adapter(&harness, adapter.clone());
        let recovery_calls = Arc::new(AtomicUsize::new(0));
        let recovery_count = recovery_calls.clone();
        let _retry = harness
            .ctx
            .on(
                "agent/request-error",
                Arc::new(move |_, _| {
                    recovery_count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async {
                        Some(cordis::arc(Some(dsh_agent::RequestErrorAction::Retry)))
                    })
                }),
                Default::default(),
            )
            .await;
        let error_calls = Arc::new(AtomicUsize::new(0));
        let error_count = error_calls.clone();
        let _errors = harness
            .ctx
            .on(
                "agent/error",
                Arc::new(move |_, _| {
                    error_count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { None })
                }),
                Default::default(),
            )
            .await;
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            recovery_calls.load(Ordering::SeqCst),
            0,
            "terminal outcomes must not enter retry backoff"
        );
        assert_eq!(
            error_calls.load(Ordering::SeqCst),
            1,
            "terminal failures still notify listeners"
        );
        let events = harness.agent.session().events();
        assert!(events.iter().any(
            |event| event.type_ == "turn/end" && event.data["reason"]["error"]["code"] == code
        ));
        let saved = events
            .iter()
            .filter(|event| event.type_ == "assistant/message")
            .collect::<Vec<_>>();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].data["message"]["content"][0]["text"],
            "safe prefix"
        );
        assert_eq!(saved[0].data["interrupted"], true);
        assert!(
            saved[0].data["message"]["source"]
                .get("replayState")
                .is_none()
        );
        assert!(!events.iter().any(|event| event.type_ == "tool/call"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_stops_empty_and_truncated_response_recovery() {
    for reply in [Reply::Empty, Reply::Limit("partial answer")] {
        let harness = harness().await;
        let mut adapter = Adapter::new(vec![reply]);
        adapter.pending_on = Some(1);
        let adapter = Arc::new(adapter);
        register_adapter(&harness, adapter.clone());
        harness.agent.followup(message("work"));
        tokio::time::timeout(Duration::from_secs(3), adapter.entered.notified())
            .await
            .unwrap();
        harness.agent.cancel(AgentCancelCause::User, None);
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        assert_eq!(turn_end_kinds(&harness.agent), ["aborted"]);
        assert!(!harness.agent.inbox().has_pending());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commentary_cannot_stand_in_for_an_explicit_empty_final_answer() {
    let harness = harness().await;
    let adapter = Arc::new(Adapter::new(vec![
        Reply::EmptyFinalAfterCommentary,
        Reply::Final("Verified result"),
    ]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(turn_end_kinds(&harness.agent), ["completed"]);
    assert!(
        adapter.histories.lock().unwrap()[1]
            .iter()
            .any(|text| text == "Checking the file.")
    );
    let harness = super::support::harness().await;
    let adapter = Arc::new(Adapter::new(vec![Reply::UnphasedWithFalseFinalHint]));
    register_adapter(&harness, adapter.clone());
    harness.agent.followup(message("work"));
    tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(
        adapter.calls.load(Ordering::SeqCst),
        1,
        "unphased text cannot acquire phase semantics from a bare hint"
    );
}
