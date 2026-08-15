//! Runtime-context projection tests: Rust port of
//! `packages/core/agent-loop/tests/runtime-context.spec.ts` (restore,
//! other-session isolation, and the cleared marker).

use cordis::Context;
use dsh_agent_loop::RuntimeContextProjection;
use dsh_llm::{ContextForm, ContentBlock, MessageSource, create_user_message};
use dsh_session::{
    SessionStore, SurfaceIntent, SurfaceOp, session_id,
};

const SOURCE: &str = "@deepseek-ai/dsh-system-prompt";

fn context_message(text: &str) -> dsh_llm::UserMessage {
    create_user_message(
        vec![ContentBlock::Text { text: text.to_string() }],
        MessageSource::Plugin {
            plugin: SOURCE.to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    )
}

fn append_intent() -> SurfaceIntent {
    SurfaceIntent { surface_op: SurfaceOp::Append, source_event_seqs: None }
}

fn message_data(message: &dsh_llm::UserMessage) -> serde_json::Value {
    serde_json::to_value(message).expect("message")
}

#[tokio::test]
async fn restores_the_latest_visible_owned_snapshot_and_ignores_other_sessions() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store
        .create(&ctx, Some(session_id("runtime-context-replay")), None)
        .await
        .expect("session");

    let retained = session
        .append("user/message", message_data(&context_message("retained")), Some(append_intent()))
        .expect("append");
    let shadowed = session
        .append("user/message", message_data(&context_message("shadowed")), Some(append_intent()))
        .expect("append");
    session
        .append(
            "user/message",
            message_data(&create_user_message(
                vec![ContentBlock::Text { text: "summary".to_string() }],
                MessageSource::Plugin {
                    plugin: "test-compaction".to_string(),
                    form: None,
                    sections: None,
                    summary: None,
                    compaction_id: None,
                    source_command_id: None,
                },
            )),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace { start: shadowed.seq, end: shadowed.seq },
                source_event_seqs: Some(vec![shadowed.seq]),
            }),
        )
        .expect("replace");

    let projection = RuntimeContextProjection::new(&ctx, &session);
    let surface_nodes: Vec<u64> = session.surface().expect("surface").nodes;
    assert!(surface_nodes.contains(&retained.seq));

    // The retained snapshot matches: no re-projection needed.
    assert!(projection.project("retained", &[]).is_none());
    // A changed snapshot projects with its named sections.
    let next = projection
        .project("next", &[dsh_llm::ContextSnapshotSection {
            name: "sandbox:policy".to_string(),
            text: "policy".to_string(),
        }])
        .expect("next snapshot");
    assert_eq!(
        next.source,
        MessageSource::Plugin {
            plugin: SOURCE.to_string(),
            form: Some(ContextForm::Snapshot),
            sections: Some(vec![dsh_llm::ContextSnapshotSection {
                name: "sandbox:policy".to_string(),
                text: "policy".to_string(),
            }]),
            summary: None,
            compaction_id: None,
            source_command_id: None,
        }
    );

    // Another session's activity does not disturb the retained snapshot.
    let other = store
        .create(&ctx, Some(session_id("runtime-context-other")), None)
        .await
        .expect("other session");
    other
        .append("user/message", message_data(&context_message("other")), Some(append_intent()))
        .expect("append");
    assert!(projection.project("retained", &[]).is_none());
}

#[tokio::test]
async fn cleared_marker_replaces_the_retained_snapshot() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let session = store.create(&ctx, Some(session_id("runtime-context-clear")), None).await.expect("session");
    session
        .append("user/message", message_data(&context_message("kept")), Some(append_intent()))
        .expect("append");

    let projection = RuntimeContextProjection::new(&ctx, &session);
    assert!(projection.project("kept", &[]).is_none());
    let cleared = projection.project("", &[]).expect("cleared marker");
    assert_eq!(
        cleared.content,
        vec![ContentBlock::Text {
            text: "Current runtime context: none. Earlier runtime-context snapshots no longer apply."
                .to_string()
        }]
    );
    assert_eq!(
        cleared.source,
        MessageSource::Plugin {
            plugin: SOURCE.to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        }
    );
}
