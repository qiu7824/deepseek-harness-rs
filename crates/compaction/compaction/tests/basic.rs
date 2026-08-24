use std::sync::Arc;

use cordis::Context;
use dsh_compaction::{BasicCompactionEngine, CompactionAgentContext, CompactionEngine};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmProviderInfo,
    LlmRuntime, MessageSource, StreamChunk, create_user_message,
};
use dsh_session::{
    CreateSessionOptions, SessionStore, SurfaceIntent, SurfaceOp, derive_event_message,
};

struct SummaryAdapter;
struct FailingAdapter;

#[async_trait::async_trait]
impl LlmAdapter for SummaryAdapter {
    fn provider_info(&self, provider: &str) -> LlmProviderInfo {
        LlmProviderInfo {
            id: provider.to_string(),
            name: provider.to_string(),
        }
    }

    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        assert_eq!(options.purpose.as_deref(), Some("compaction"));
        let block = ContentBlock::Text {
            text: "## Current Work\n- continue implementation".to_string(),
        };
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockEnd { index: 0, block },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

#[async_trait::async_trait]
impl LlmAdapter for FailingAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::iter(vec![StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: dsh_llm::LlmFailure {
                    message: "summary failed".to_string(),
                    code: "SUMMARY_FAILED".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        }]))
    }
}

#[tokio::test]
async fn manual_region_calls_llm_and_replaces_surface_with_checkpoint() {
    let ctx = Context::root();
    let sessions = SessionStore::install(&ctx);
    dsh_token_meter::TokenMeter::install(&ctx, Default::default());
    let llm = LlmRuntime::install(&ctx);
    llm.register_adapter(&ctx, vec!["test".to_string()], Arc::new(SummaryAdapter))
        .expect("adapter");
    let engine = BasicCompactionEngine::install(&ctx, 512).expect("engine");
    let session = sessions
        .create(&ctx, None, Some(CreateSessionOptions::default()))
        .await
        .expect("session");
    for text in ["first", "second"] {
        let message = create_user_message(
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            MessageSource::Plugin {
                plugin: "test".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .expect("message");
    }
    let before = session.surface().unwrap().nodes;
    let result = engine
        .compact_region(
            before[0],
            before[0],
            &CompactionAgentContext {
                session: session.clone(),
                provider: Some("test".to_string()),
                model: Some("summary".to_string()),
            },
            None,
        )
        .await
        .expect("compact");
    assert_eq!(result.shadowed_seqs, vec![before[0]]);
    let events = session.events();
    let types: Vec<_> = events.iter().map(|event| event.type_.as_str()).collect();
    assert!(types.windows(4).any(|window| {
        window
            == [
                "compaction/start",
                "compaction/summary",
                "user/message",
                "compaction/end",
            ]
    }));
    let surface = session.surface().unwrap().nodes;
    assert_eq!(surface.len(), 2);
    let checkpoint = derive_event_message(&events[surface[0] as usize]).expect("checkpoint");
    assert!(checkpoint.content.iter().any(|block| {
        block
            .as_text()
            .is_some_and(|text| text.contains("compacted-summary"))
    }));
}

#[tokio::test]
async fn summary_failure_closes_lifecycle_without_replacing_surface() {
    let ctx = Context::root();
    let sessions = SessionStore::install(&ctx);
    dsh_token_meter::TokenMeter::install(&ctx, Default::default());
    let llm = LlmRuntime::install(&ctx);
    llm.register_adapter(&ctx, vec!["fail".to_string()], Arc::new(FailingAdapter))
        .expect("adapter");
    let engine = BasicCompactionEngine::install(&ctx, 512).expect("engine");
    let session = sessions
        .create(&ctx, None, Some(CreateSessionOptions::default()))
        .await
        .expect("session");
    for text in ["first", "second"] {
        let message = create_user_message(
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            MessageSource::Plugin {
                plugin: "test".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        );
        session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .expect("message");
    }
    let before = session.surface().unwrap().nodes;
    let error = engine
        .compact_region(
            before[0],
            before[0],
            &CompactionAgentContext {
                session: session.clone(),
                provider: Some("fail".to_string()),
                model: Some("summary".to_string()),
            },
            None,
        )
        .await
        .expect_err("summary failure");
    assert_eq!(error.code.as_str(), "summary");
    assert_eq!(session.surface().unwrap().nodes, before);
    let events = session.events();
    let types: Vec<_> = events.iter().map(|event| event.type_.as_str()).collect();
    assert!(types.ends_with(&["compaction/start", "compaction/end"]));
    assert!(!types.contains(&"compaction/summary"));
}
