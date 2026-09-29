use super::*;
use dsh_llm::{ChunkStream, LlmAdapter, StreamChunk};
use dsh_session::{SessionEvent, SessionLogOffset, format_v4::V4Validator};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct SummaryAdapter(AtomicUsize);
impl LlmAdapter for SummaryAdapter {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter([
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "Keep the task requirements and continue the pending work.".into(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

fn parent() -> Session {
    let session = Session::create(
        dsh_session::session_id("compaction-parent"),
        None,
        None,
        None,
    )
    .unwrap();
    for text in [
        "First requirement",
        "Second requirement",
        "Continue the task",
    ] {
        let message = create_user_message(
            vec![ContentBlock::Text { text: text.into() }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
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
            .unwrap();
    }
    session
        .append(
            "compaction/start",
            json!({"compactionId":"interrupted-parent","turn":null}),
            None,
        )
        .unwrap();
    session
}

fn fork(parent: &Session) -> Session {
    let mut header = parent.header().clone();
    header.id = dsh_session::session_id("compaction-child");
    header.parent_session = Some(parent.id().clone());
    header.is_seeded = true;
    Session::create(
        header.id.clone(),
        Some(parent.events().as_ref().clone()),
        Some(&header),
        Some(parent.seq()),
    )
    .unwrap()
}

fn cold_restore(session: &Session, closers: Vec<SessionEvent>) -> Session {
    let mut builder =
        dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
    session
        .visit_events(0, None, |event| {
            builder.push(event)?;
            Ok(true)
        })
        .unwrap();
    Session::from_event_archive(
        session.id().clone(),
        builder.finish().unwrap(),
        session.header(),
        session.inherited_event_count(),
        closers,
    )
    .unwrap()
}

fn validate(session: &Session) {
    let mut validator = V4Validator::new(
        serde_json::to_value(session.header()).unwrap(),
        session.inherited_event_count().get(),
    )
    .unwrap();
    session
        .visit_events(0, None, |event| {
            validator.push(&serde_json::to_value(event).unwrap())?;
            Ok(true)
        })
        .unwrap();
    assert!(!validator.finish().unwrap().open_compaction);
}

async fn compact(session: Session, should_succeed: bool) {
    let ctx = cordis::Context::root();
    let llm = LlmRuntime::install(&ctx);
    let sessions = SessionStore::install(&ctx);
    let _session = sessions.enter(&session).unwrap();
    TokenMeter::install(&ctx, Default::default());
    let adapter = Arc::new(SummaryAdapter::default());
    let _adapter = llm
        .register_adapter(&ctx, vec!["fixture".into()], adapter.clone())
        .unwrap();
    let engine = BasicCompactionEngine::install(&ctx, 128).unwrap();
    let before = session.events();
    let result = engine
        .compact_now(
            &ManualCompactAgentContext {
                session: session.clone(),
                provider: Some("fixture".into()),
                model: Some("fixture".into()),
            },
            None,
            None,
        )
        .await;
    if should_succeed {
        let result = result
            .unwrap()
            .expect("older messages must produce a checkpoint");
        assert!(!result.summary.is_empty());
        assert_eq!(adapter.0.load(Ordering::SeqCst), 1);
        validate(&session);
        let cold = cold_restore(&session, vec![]);
        validate(&cold);
        assert_eq!(
            cold.surface().unwrap().nodes,
            session.surface().unwrap().nodes
        );
    } else {
        assert_eq!(result.unwrap_err().code, ManualCompactionErrorCode::Busy);
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
    }
    assert_eq!(&session.events()[..before.len()], before.as_slice());
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
}

#[tokio::test]
async fn inherited_unfinished_compaction_expires_for_live_and_cold_children() {
    for cold in [false, true] {
        let parent = parent();
        let parent_before = parent.events();
        let child = fork(&parent);
        assert_eq!(
            child.inherited_event_count(),
            SessionLogOffset::new(parent_before.len() as u64).unwrap()
        );
        validate(&child);
        let child = if cold {
            cold_restore(&child, vec![])
        } else {
            child
        };
        compact(child, true).await;
        assert_eq!(parent.events().as_slice(), parent_before.as_slice());
    }
}

#[tokio::test]
async fn a_new_compaction_after_the_seed_boundary_still_blocks_overlap() {
    let child = fork(&parent());
    child
        .append(
            "compaction/start",
            json!({"compactionId":"active-child","turn":null}),
            None,
        )
        .unwrap();
    compact(child, false).await;
}

#[tokio::test]
async fn interrupted_summary_is_closed_before_cold_restore_and_can_compact_again() {
    let parent = parent();
    let original = parent.events();
    let closers = dsh_session::interrupted_turn_closers(&original);
    assert_eq!(closers.len(), 1);
    assert_eq!(closers[0].type_, "compaction/end");
    let restored = cold_restore(&parent, closers);
    validate(&restored);
    compact(restored, true).await;
    assert_eq!(parent.events().as_slice(), original.as_slice());
}
