use std::sync::Arc;

use cordis::Context;
use dsh_agent::{AgentFactory, AgentOptions};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id,
    create_user_message,
};
use dsh_session::session_id;

struct RunCodeThenTextAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for RunCodeThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let chunks = if first {
            let arguments = serde_json::json!({
                "code": "const result = await tools.pwsh({ command: \"Write-Output 'host-code-mode-e2e'\", description: \"Print code-mode marker\" }); return result.stdout;",
                "description": "Run PowerShell through Code Mode"
            })
            .to_string();
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: call_id("host-run-code-call"),
                    name: Some("run_code".to_string()),
                    arguments_delta: arguments.clone(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: call_id("host-run-code-call"),
                        name: "run_code".to_string(),
                        arguments,
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "code mode completed".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "code mode completed".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(chunks))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_runs_code_mode_through_a_real_nested_pwsh_tool() {
    let ctx = Context::root();
    let spine = dsh_host::compose_host(&ctx).expect("compose host");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-code".to_string()],
            Arc::new(RunCodeThenTextAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("register adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-code-mode")),
                agent_options: Some(AgentOptions {
                    provider: Some("test-code".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_agent_tool_presentation::apply(
        handle.agent.ctx(),
        dsh_agent_tool_presentation::Config {
            mode: dsh_tools::ToolPresentationMode::Code,
        },
    )
    .expect("declare code presentation");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");

    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Use Code Mode to print the marker.".to_string(),
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
    assert!(results[0].to_string().contains("host-code-mode-e2e"));

    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");
}
