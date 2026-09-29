use super::support::{harness, message, register_adapter};
use dsh_agent::Agent;
use dsh_llm::{ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk};
use dsh_system_prompt::{PromptContext, PromptText, SystemPrompt};
use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Payloads(Arc<Mutex<Vec<Vec<u8>>>>);
impl LlmAdapter for Payloads {
    fn stream(&self, request: &GenerateOptions) -> ChunkStream {
        // These are the actual assembled messages handed to the adapter;
        // source/projection metadata is excluded from model-facing payloads.
        let messages: Vec<_> = request
            .messages
            .iter()
            .map(|message| serde_json::json!({"role":message.role,"content":message.content}))
            .collect();
        self.0
            .lock()
            .push(serde_json::to_vec(&serde_json::json!({"messages":messages})).unwrap());
        Box::pin(futures::stream::iter([
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text { text: "ok".into() },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

#[tokio::test]
async fn real_request_payload_keeps_one_full_runtime_baseline_across_updates() {
    let h = harness().await;
    let payloads = Arc::new(Mutex::new(Vec::new()));
    register_adapter(&h, Arc::new(Payloads(payloads.clone())));
    let prompt = h
        .ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .unwrap();
    let progress = Arc::new(AtomicUsize::new(0));
    let live = progress.clone();
    prompt.context(
        &h.ctx,
        PromptContext {
            name: "document:rules".into(),
            order: 1.0,
            text: PromptText::Static(format!(
                "STABLE_DOCUMENT_RULES_MARKER {}",
                "原始字符串必须保留。".repeat(6000)
            )),
        },
    );
    prompt.context(
        &h.ctx,
        PromptContext {
            name: "document:progress".into(),
            order: 2.0,
            text: PromptText::Provider(Arc::new(move |_| {
                format!("Current completed rows: {}", live.load(Ordering::SeqCst))
            })),
        },
    );
    for index in 0..3 {
        if index == 1 {
            progress.store(1, Ordering::SeqCst);
        }
        h.agent
            .followup(message("Continue with the same document."));
        tokio::time::timeout(std::time::Duration::from_secs(5), h.agent.when_idle())
            .await
            .unwrap();
    }
    let rows = payloads.lock();
    assert_eq!(rows.len(), 3);
    for payload in rows.iter() {
        assert_eq!(
            String::from_utf8_lossy(payload)
                .matches("STABLE_DOCUMENT_RULES_MARKER")
                .count(),
            1
        );
    }
    assert!(
        rows[1].len() < rows[0].len() + 2000,
        "changing one section must not append a second full context: {} -> {}",
        rows[0].len(),
        rows[1].len()
    );
    assert!(
        rows[2].len() < rows[1].len() + 1000,
        "unchanged facts add no snapshot"
    );
    assert!(
        String::from_utf8_lossy(&rows[1]).contains("Only the named sections")
            || String::from_utf8_lossy(&rows[1]).contains("only the named sections")
    );
    println!(
        "RUNTIME_PAYLOAD_BYTES={:?}",
        rows.iter().map(Vec::len).collect::<Vec<_>>()
    );
    drop(rows);
    let events = h.agent.session().events();
    let contexts: Vec<_> = events
        .iter()
        .filter(|event| {
            event.type_ == "user/message" && event.data["source"]["kind"] == "runtime-context"
        })
        .collect();
    assert_eq!(contexts.len(), 2);
    assert_eq!(
        contexts[1].data["source"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    h.ctx.fiber.dispose().await;
}
