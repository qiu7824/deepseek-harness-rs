use std::sync::Arc;

use cordis::Context;
use dsh_agent::{AgentFactory, AgentOptions};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id,
    create_user_message,
};
use dsh_session::session_id;

struct RalphAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for RalphAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let chunks = match call {
            0 => tool_chunks(
                "ralph-call",
                "ralph",
                serde_json::json!({
                    "objective": "Complete the one-round migration",
                    "maxRounds": 1
                }),
            ),
            1 => tool_chunks(
                "structured-call",
                "structured_output",
                serde_json::json!({
                    "status": "complete",
                    "summary": "Migration complete",
                    "evidence": ["host Ralph E2E marker"],
                    "nextSteps": [],
                    "blocker": ""
                }),
            ),
            _ => text_chunks("ralph completed"),
        };
        Box::pin(futures::stream::iter(chunks))
    }
}

fn tool_chunks(id: &str, name: &str, arguments: serde_json::Value) -> Vec<StreamChunk> {
    let arguments = arguments.to_string();
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "tool-call".to_string(),
        },
        StreamChunk::ToolCallDelta {
            index: 0,
            id: call_id(id),
            name: Some(name.to_string()),
            arguments_delta: arguments.clone(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::ToolCall {
                id: call_id(id),
                name: name.to_string(),
                arguments,
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::ToolCalls,
            replay_state: None,
        },
    ]
}

fn text_chunks(text: &str) -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::TextDelta {
            index: 0,
            text: text.to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: text.to_string(),
            },
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_runs_one_fresh_structured_ralph_round() {
    let ctx = Context::root();
    let spine = dsh_host::compose_host(&ctx).expect("compose host");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-ralph".to_string()],
            Arc::new(RalphAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("register adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id(format!("host-ralph-{}", uuid::Uuid::new_v4()))),
                agent_options: Some(AgentOptions {
                    provider: Some("test-ralph".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create parent agent");

    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Run one Ralph round.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;

    let results: Vec<_> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].get("error").is_none(), "{results:?}");
    assert!(
        results[0].to_string().contains("host Ralph E2E marker"),
        "{results:?}"
    );

    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");
}
