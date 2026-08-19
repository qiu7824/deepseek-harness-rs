//! All-messages LLM title provider end-to-end. Rust port of
//! `packages/session/session-title-all-prompts-llm/tests/provider.spec.ts`.

use std::sync::Arc;

use cordis::Context;
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter, LlmRuntime, StreamChunk};
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_title::{Config as TitleConfig, SessionTitleService};
use dsh_session_title_all_prompts_llm::{Config as LlmConfig, SessionTitleAllPromptsLlmPlugin};
use dsh_session_title_llm::resolve_session_title_llm_config;

struct RecordingAdapter {
    requests: parking_lot::Mutex<Vec<GenerateOptions>>,
}

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.requests.lock().push(options.clone());
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".to_string(),
            },
            StreamChunk::TextDelta {
                index: 0,
                text: "All messages model title".to_string(),
            },
            StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

const TITLE_CONFIG: TitleConfig = TitleConfig {
    fallback_max_words: 5,
    fallback_max_bytes: 40,
    max_title_bytes: 80,
};

const LLM_CONFIG: &str = r#"{
    "targetWords": 5,
    "targetCjkCharacters": 10,
    "maxInputBytes": 1000,
    "maxOutputTokens": 32,
    "timeoutMs": 1000
}"#;

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn includes_seeded_history_and_the_latest_prompt_while_inheriting_the_logged_request_route() {
    let seeded = Session::create(session_id("seed-source"), None, None).expect("create");
    seeded
        .append("turn/start", dsh_session::turn_start_data(1), None)
        .expect("turn/start");
    let inherited = seeded
        .append(
            "user/message",
            serde_json::json!({
                "id": "i1",
                "role": "user",
                "content": [{"type": "text", "text": "inherited prompt"}],
                "source": {"kind": "user"},
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append");
    seeded
        .append(
            "session/title",
            serde_json::json!({
                "title": "Inherited fallback",
                "messageSeqs": [inherited.seq],
                "source": {"kind": "fallback"},
            }),
            None,
        )
        .expect("append");
    seeded
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("turn/end");

    let ctx = Context::root();
    let llm = LlmRuntime::install(&ctx);
    let store = SessionStore::install(&ctx);
    let _title_service = SessionTitleService::install(&ctx, TITLE_CONFIG).expect("title service");
    let adapter = Arc::new(RecordingAdapter {
        requests: parking_lot::Mutex::new(Vec::new()),
    });
    llm.register_adapter(&ctx, vec!["current-route".to_string()], adapter.clone())
        .expect("adapter");
    let llm_config: LlmConfig =
        resolve_session_title_llm_config(&serde_json::from_str(LLM_CONFIG).unwrap())
            .expect("llm config");
    let fiber = ctx.plugin(
        Arc::new(SessionTitleAllPromptsLlmPlugin),
        cordis::arc(llm_config),
    );
    fiber.settle().await.expect("settle");

    let seed_events = seeded.events().as_ref().clone();
    let session = store
        .create(
            &ctx,
            Some(session_id("all-plugin")),
            Some(dsh_session::CreateSessionOptions {
                seed: Some(seed_events),
                meta: Some(dsh_session::CreateSessionMeta {
                    parent_session: Some(seeded.id().clone()),
                    seed_length: Some(seeded.seq() as u64),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("create");
    session
        .append("turn/start", dsh_session::turn_start_data(2), None)
        .expect("turn/start");
    let latest = session
        .append(
            "user/message",
            serde_json::json!({
                "id": "l1",
                "role": "user",
                "content": [{"type": "text", "text": "latest prompt"}],
                "source": {"kind": "user"},
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append");
    settle().await;
    session
        .append(
            "request/header",
            serde_json::json!({
                "header": {"config": {"provider": "current-route", "model": "current-model"}},
                "reason": "resume",
            }),
            None,
        )
        .expect("append");
    settle().await;

    let requests = adapter.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].provider, "current-route");
    assert_eq!(requests[0].model, "current-model");
    let prompt = match &requests[0].messages[0].content[0] {
        dsh_llm::ContentBlock::Text { text } => text.clone(),
        other => panic!("expected text, got {other:?}"),
    };
    assert!(prompt.contains("inherited prompt"), "{prompt}");
    assert!(prompt.contains("latest prompt"), "{prompt}");
    drop(requests);

    let service = ctx
        .get_typed::<Arc<SessionTitleService>>("sessionTitle", false)
        .expect("service")
        .as_ref()
        .clone();
    let snapshot = service.get(&session).expect("title");
    assert_eq!(snapshot.message_seqs, vec![inherited.seq, latest.seq]);
}
