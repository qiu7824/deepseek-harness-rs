use std::sync::Arc;

use cordis::Context;
use dsh_agent::{AgentFactory, AgentOptions};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id,
    create_user_message,
};
use dsh_session::session_id;

struct WorkflowAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for WorkflowAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let chunks = match call {
            0 => {
                let arguments = serde_json::json!({
                    "script": "return await agent({ prompt: 'Reply with exactly workflow-child-marker', label: 'marker child' });",
                    "meta": {
                        "name": "marker-workflow",
                        "description": "Run one fresh child and return its marker"
                    }
                })
                .to_string();
                vec![
                    StreamChunk::BlockStart {
                        index: 0,
                        block_type: "tool-call".to_string(),
                    },
                    StreamChunk::ToolCallDelta {
                        index: 0,
                        id: call_id("host-workflow-call"),
                        name: Some("workflow".to_string()),
                        arguments_delta: arguments.clone(),
                    },
                    StreamChunk::BlockEnd {
                        index: 0,
                        block: ContentBlock::ToolCall {
                            id: call_id("host-workflow-call"),
                            name: "workflow".to_string(),
                            arguments,
                        },
                    },
                    StreamChunk::Finish {
                        reason: FinishReason::ToolCalls,
                        replay_state: None,
                    },
                ]
            }
            1 => text_chunks("workflow-child-marker"),
            _ => text_chunks("workflow completed"),
        };
        Box::pin(futures::stream::iter(chunks))
    }
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
async fn production_host_runs_workflow_through_a_fresh_child_and_back() {
    let ctx = Context::root();
    let spine = dsh_host::compose_host(&ctx).expect("compose host");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-workflow".to_string()],
            Arc::new(WorkflowAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("register adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id(format!(
                    "host-workflow-{}",
                    uuid::Uuid::new_v4()
                ))),
                agent_options: Some(AgentOptions {
                    provider: Some("test-workflow".to_string()),
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
            text: "Run the marker workflow.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;

    let events = handle.agent.session().events();
    let results: Vec<_> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].get("error").is_none(), "{results:?}");
    assert!(results[0].to_string().contains("workflow-child-marker"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool-workflow/run-start")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool-workflow/run-end")
            .count(),
        1
    );

    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");
}
