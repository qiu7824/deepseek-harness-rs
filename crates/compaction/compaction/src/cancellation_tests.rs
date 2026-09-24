use super::*;
use dsh_llm::{ChunkStream, LlmAdapter, LlmFailure, StreamChunk};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct CancelledStream {
    cancelled: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl LlmAdapter for CancelledStream {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(true, Ordering::SeqCst);
        Box::pin(futures::stream::iter([StreamChunk::Finish {
            reason: FinishReason::Error {
                failure: LlmFailure {
                    message: "DeepSeek stream cancelled".into(),
                    code: "CANCELLED".into(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                    offload_images: None,
                },
            },
            replay_state: None,
        }]))
    }
}

#[tokio::test]
async fn provider_cancellation_before_the_poll_tick_is_not_a_summary_quality_failure() {
    let ctx = cordis::Context::root();
    let llm = LlmRuntime::install(&ctx);
    let sessions = SessionStore::install(&ctx);
    TokenMeter::install(&ctx, Default::default());
    let cancelled = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let _adapter = llm
        .register_adapter(
            &ctx,
            vec!["fixture".into()],
            Arc::new(CancelledStream {
                cancelled: cancelled.clone(),
                calls: calls.clone(),
            }),
        )
        .unwrap();
    let session = sessions.create(&ctx, None, None).await.unwrap();
    let engine = BasicCompactionEngine::install(&ctx, 1024).unwrap();
    let agent = CompactionAgentContext {
        session,
        provider: Some("fixture".into()),
        model: Some("fixture".into()),
    };
    let signal: CompactionAbort = Arc::new(move || cancelled.load(Ordering::SeqCst));
    let result = engine.summarize(&agent, vec![], Some(&signal)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        result.unwrap_err().code,
        ManualCompactionErrorCode::Cancelled
    );
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
}
