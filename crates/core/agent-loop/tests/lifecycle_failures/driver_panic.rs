use super::support::{harness, message, register_adapter};
use dsh_agent::{Agent, AgentStatus};
use dsh_llm::{ChunkStream, FinishReason, GenerateOptions, LlmAdapter, StreamChunk};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct ReplyAdapter(AtomicUsize);
impl LlmAdapter for ReplyAdapter {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter([
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::TextDelta {
                index: 0,
                text: "recovered".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: dsh_llm::ContentBlock::Text {
                    text: "recovered".into(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_panic_closes_turn_and_accepts_the_next_prompt() {
    let h = harness().await;
    let adapter = Arc::new(ReplyAdapter(AtomicUsize::new(0)));
    register_adapter(&h, adapter.clone());
    let attempts = Arc::new(AtomicUsize::new(0));
    h.ctx
        .on(
            "agent/pre-step",
            Arc::new(move |_, args| {
                let attempts = attempts.clone();
                Box::pin(async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("pre-step hook failed");
                    }
                    let next =
                        cordis::downcast_arc::<cordis::NextFn>(args.last().unwrap()).unwrap();
                    Some(next.call().await)
                })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    h.agent.followup(message("first"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(h.agent.status(), AgentStatus::Idle);
    let events = h.agent.session().events();
    let end = events.iter().rev().find(|e| e.type_ == "turn/end").unwrap();
    assert_eq!(end.data["reason"]["kind"], "error");
    assert_eq!(end.data["reason"]["error"]["code"], "DRIVER_PANIC");
    h.agent.followup(message("second"));
    tokio::time::timeout(Duration::from_secs(2), h.agent.when_idle())
        .await
        .unwrap();
    assert_eq!(adapter.0.load(Ordering::SeqCst), 1);
    assert_eq!(h.agent.status(), AgentStatus::Idle);
}
