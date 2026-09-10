use super::support::{Harness, harness, message, register_adapter};
use dsh_agent::Agent;
use dsh_compaction::{
    CompactionAgentContext, CompactionEngine, CompactionTrigger, ManualCompactAgentContext,
    ManualCompactionErrorCode, basic::BasicCompactionEngine,
};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, Role, StreamChunk,
};
use dsh_session::{Session, SurfaceIntent, SurfaceOp, session_id};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

struct RecordingAdapter(Arc<Mutex<Vec<GenerateOptions>>>);

impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        self.0.lock().push(options.clone());
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "Retained conversation checkpoint.".into(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

async fn setup() -> (
    Harness,
    Arc<BasicCompactionEngine>,
    Arc<Mutex<Vec<GenerateOptions>>>,
) {
    let harness = harness().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    register_adapter(&harness, Arc::new(RecordingAdapter(calls.clone())));
    dsh_token_meter::TokenMeter::install(&harness.ctx, Default::default());
    let engine = BasicCompactionEngine::install(&harness.ctx, 1024).unwrap();
    (harness, engine, calls)
}

async fn turn(harness: &Harness, text: &str) {
    harness.agent.followup(message(text));
    tokio::time::timeout(Duration::from_secs(5), harness.agent.when_idle())
        .await
        .unwrap();
}

fn context(harness: &Harness) -> CompactionAgentContext {
    CompactionAgentContext {
        session: harness.agent.session().clone(),
        provider: Some("test".into()),
        model: Some("model".into()),
    }
}

fn assert_prefix(session: &Session, expected: &[ContentBlock]) {
    let messages = session.derive_messages().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == Role::System)
            .count(),
        if expected.is_empty() { 0 } else { 1 }
    );
    if !expected.is_empty() {
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content, expected);
    }
    // Fork/reload must derive the same prefix and preserve conversation order.
    let fork = Session::create(
        session_id(uuid::Uuid::new_v4().to_string()),
        Some(session.events().to_vec()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(fork.derive_messages().unwrap().as_ref(), messages.as_ref());
    let mut streaming = dsh_session::surface::StreamingSurfaceFold::default();
    let mut meter_nodes = Vec::new();
    for event in session.events().iter() {
        streaming.push(event).unwrap();
        meter_nodes = dsh_token_meter::fold_surface_tokens(&meter_nodes, event)
            .unwrap()
            .nodes;
    }
    let expected_nodes = session.surface().unwrap().nodes;
    assert_eq!(streaming.finish().nodes, expected_nodes);
    assert_eq!(
        meter_nodes.iter().map(|node| node.seq).collect::<Vec<_>>(),
        expected_nodes
    );
}

#[tokio::test]
async fn real_manual_and_automatic_compaction_preserve_v3_prefix_across_repeated_ranges() {
    let (harness, engine, calls) = setup().await;
    turn(&harness, "Remember the original goal.").await;
    turn(&harness, "Continue the same goal.").await;
    let session = harness.agent.session();
    let prefix = session.derive_messages().unwrap()[0].content.clone();
    let nodes = session.surface().unwrap().nodes;
    let system_seq = nodes[0];
    let manual = ManualCompactAgentContext {
        session: session.clone(),
        provider: Some("test".into()),
        model: Some("model".into()),
    };
    let first = engine
        .compact_region(nodes[1], nodes[2], &context(&harness), None)
        .await
        .unwrap();
    assert!(!first.shadowed_seqs.contains(&system_seq));
    assert_prefix(session, &prefix);
    let second = engine
        .compact_now(&manual, None, None)
        .await
        .unwrap()
        .unwrap();
    // The previous replacement is newer in log order than its following nodes.
    assert!(second.shadowed_range.0 > second.shadowed_range.1);
    assert!(!second.shadowed_seqs.contains(&system_seq));
    assert_prefix(session, &prefix);
    turn(&harness, "Continue after the checkpoint.").await;
    let third = engine
        .compact_if_needed(&context(&harness), CompactionTrigger::ContextOverflow, None)
        .await
        .unwrap()
        .unwrap();
    assert!(!third.shadowed_seqs.contains(&system_seq));
    assert_prefix(session, &prefix);
    turn(&harness, "Continue after repeated compaction.").await;
    assert_prefix(session, &prefix);
    let calls = calls.lock();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.purpose.as_deref() == Some("compaction"))
            .count(),
        3
    );
    for request in calls.iter().filter(|call| call.agent_loop_request) {
        assert!(request.system.is_none());
        assert_eq!(request.messages[0].role, Role::System);
        assert_eq!(request.messages[0].content, prefix);
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|message| message.role == Role::System)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn explicit_compaction_rejects_system_ranges_before_summarization_or_log_changes() {
    let (harness, engine, calls) = setup().await;
    let prompt = harness
        .ctx
        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        .unwrap();
    let _section = prompt.section(
        &harness.ctx,
        dsh_system_prompt::PromptSection {
            name: "compaction-protected-system".into(),
            order: 0.0,
            text: dsh_system_prompt::PromptText::Static(
                "Keep the protected system instruction.".into(),
            ),
            complete: Some(true),
        },
    );
    turn(&harness, "Keep the instructions.").await;
    let session = harness.agent.session();
    let nodes = session.surface().unwrap().nodes;
    let event_count = session.events().len();
    let call_count = calls.lock().len();
    for end in [nodes[0], *nodes.last().unwrap()] {
        let error = engine
            .compact_region(nodes[0], end, &context(&harness), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, ManualCompactionErrorCode::Commit);
        assert!(error.message.contains("protected system head"), "{}", error.message);
        assert_eq!(session.events().len(), event_count);
        assert_eq!(calls.lock().len(), call_count);
    }
}

#[tokio::test]
async fn old_shadowed_system_is_restored_once_and_dynamic_changes_replace_live_node() {
    let (harness, _, calls) = setup().await;
    let text = Arc::new(Mutex::new("System rule A".to_string()));
    let prompt = harness
        .ctx
        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        .unwrap();
    let supplied = text.clone();
    let _section = prompt.section(
        &harness.ctx,
        dsh_system_prompt::PromptSection {
            name: "test-system-prefix".into(),
            order: 0.0,
            text: dsh_system_prompt::PromptText::Provider(Arc::new(move |_| {
                supplied.lock().clone()
            })),
            complete: Some(true),
        },
    );
    turn(&harness, "First turn.").await;
    let session = harness.agent.session();
    let prefix = session.derive_messages().unwrap()[0].content.clone();
    let nodes = session.surface().unwrap().nodes;
    // Reproduce the durable shape written by the old compactor: the system
    // and conversation are replaced together by one checkpoint user message.
    session
        .append(
            "user/message",
            serde_json::to_value(message("Legacy checkpoint.")).unwrap(),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace {
                    start: nodes[0],
                    end: *nodes.last().unwrap(),
                },
                source_event_seqs: Some(nodes.clone()),
            }),
        )
        .unwrap();
    assert!(
        session
            .derive_messages()
            .unwrap()
            .iter()
            .all(|message| message.role != Role::System)
    );
    turn(&harness, "Restore and continue.").await;
    assert_prefix(session, &prefix);
    let restored = session.surface().unwrap().nodes[0];
    assert!(restored > nodes[0]);
    let system_events = || {
        session
            .events()
            .iter()
            .filter(|event| event.type_ == "system/message")
            .count()
    };
    assert_eq!(system_events(), 2);
    turn(&harness, "Continue without changing instructions.").await;
    assert_eq!(system_events(), 2);
    *text.lock() = "System rule B".into();
    turn(&harness, "Apply the current instruction.").await;
    let live = session.surface().unwrap().nodes[0];
    let events = session.events();
    assert_eq!(
        events[live as usize].surface_op,
        Some(SurfaceOp::Replace {
            start: restored,
            end: restored
        })
    );
    assert_prefix(
        session,
        &[ContentBlock::Text {
            text: "System rule B".into(),
        }],
    );
    assert_eq!(
        calls.lock().last().unwrap().messages[1].content,
        message("Legacy checkpoint.").content
    );
    *text.lock() = String::new();
    turn(&harness, "Continue with the cleared system prompt.").await;
    assert_prefix(session, &[]);
    let after_clear = system_events();
    turn(&harness, "Continue with the same empty prompt.").await;
    assert_eq!(system_events(), after_clear);
    assert_prefix(session, &[]);
}

#[tokio::test]
async fn unmarked_v3_system_after_history_replays_old_ranges_before_prefix_repair() {
    let (harness, _, calls) = setup().await;
    let session = harness.agent.session();
    let append = || {
        Some(SurfaceIntent {
            surface_op: SurfaceOp::Append,
            source_event_seqs: None,
        })
    };
    let user = session
        .append(
            "user/message",
            serde_json::to_value(message("Legacy task.")).unwrap(),
            append(),
        )
        .unwrap();
    let system_message = dsh_llm::create_message(
        Role::System,
        vec![ContentBlock::Text {
            text: "Legacy system instruction.".into(),
        }],
        dsh_llm::MessageSource::Plugin {
            plugin: "@deepseek-ai/dsh-system-prompt".into(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    let system = session
        .append(
            "system/message",
            serde_json::json!({"message": system_message}),
            append(),
        )
        .unwrap();
    assert_eq!(
        session.surface().unwrap().nodes,
        vec![user.seq.get(), system.seq.get()]
    );
    session
        .append(
            "user/message",
            serde_json::to_value(message("Retained recent message.")).unwrap(),
            append(),
        )
        .unwrap();
    session
        .append(
            "user/message",
            serde_json::to_value(message("Legacy checkpoint.")).unwrap(),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Replace {
                    start: user.seq.get(),
                    end: system.seq.get(),
                },
                source_event_seqs: Some(vec![user.seq.get(), system.seq.get()]),
            }),
        )
        .unwrap();
    let restored = Session::create(
        session_id(uuid::Uuid::new_v4().to_string()),
        Some(session.events().to_vec()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        restored.derive_messages().unwrap().as_ref(),
        session.derive_messages().unwrap().as_ref()
    );
    turn(&harness, "Continue from the old checkpoint.").await;
    let request = calls.lock().last().unwrap().clone();
    assert_prefix(session, &request.messages[0].content);
    assert_eq!(request.messages[0].role, Role::System);
    assert_eq!(
        request.messages[1].content,
        message("Legacy checkpoint.").content
    );
    assert_eq!(
        request.messages[2].content,
        message("Retained recent message.").content
    );
    let active_system = session.surface().unwrap().nodes[0];
    assert_eq!(
        session.events()[active_system as usize].data["prefix"],
        true
    );
}
