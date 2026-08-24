use std::sync::Arc;

use cordis::Context;

use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id,
    create_user_message,
};
use dsh_session::session_id;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn rpc(port: u16, method: &str, payload: serde_json::Value) -> serde_json::Value {
    let body = serde_json::json!({
        "type": "client-request",
        "rpcId": format!("workflow-{method}"),
        "method": method,
        "payload": payload,
    })
    .to_string();
    let request = format!(
        "POST /api/{method} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect host");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write RPC");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.expect("read RPC");
    let text = String::from_utf8(bytes).expect("RPC UTF-8");
    let (head, body) = text.split_once("\r\n\r\n").expect("RPC response head");
    assert!(head.contains(" 200 "), "{head}\n{body}");
    serde_json::from_str(body).expect("RPC JSON")
}

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
    let id = format!("host-workflow-{}", uuid::Uuid::new_v4());
    let port = spine.web_server.port();
    let created = rpc(port, "session.create", serde_json::json!({"sessionId": id})).await;
    assert_eq!(created["result"]["ok"], true, "{created}");
    let selected = rpc(
        port,
        "session.selectModel",
        serde_json::json!({
            "sessionId": id,
            "provider": "test-workflow",
            "model": "model"
        }),
    )
    .await;
    assert_eq!(selected["result"]["ok"], true, "{selected}");
    let agent = spine
        .agents
        .get(&session_id(&id))
        .expect("workflow parent agent");
    agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Run the marker workflow.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    agent.when_idle().await;

    let events = agent.session().events();
    let results: Vec<_> = events
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].get("error").is_none(), "{results:?}");
    let workflow_text = results[0]
        .pointer("/message/content/0/content/0/text")
        .and_then(serde_json::Value::as_str)
        .expect("workflow tool result text");
    let workflow_result: serde_json::Value =
        serde_json::from_str(workflow_text).expect("workflow result JSON");
    assert_eq!(workflow_result["agentsStarted"], 1, "{workflow_result}");
    assert_eq!(
        workflow_result["result"], "workflow completed",
        "{workflow_result}"
    );
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

    assert!(
        spine
            .agents
            .retire(agent)
            .await
            .expect("retire workflow parent")
    );
    spine.shutdown().await.expect("shutdown");
}
