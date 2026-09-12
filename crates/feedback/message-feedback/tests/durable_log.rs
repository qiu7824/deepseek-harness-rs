use std::path::PathBuf;
use std::sync::{Arc, atomic::Ordering};

use cordis::Context;
use dsh_message_feedback::*;
use dsh_session::{SessionEvent, SessionHeader, SessionSeq, SessionStore, SurfaceOp, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use dsh_storage::Storage;
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};
use serde_json::json;

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        let base = std::env::var_os("DSH_TEST_TEMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!("feedback-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    ctx: Context,
    sessions: Arc<SessionStore>,
    persistence: Arc<JsonlSessionPersistence>,
    feedback: Arc<MessageFeedbackService>,
}
impl Fixture {
    fn new(root: &TempRoot, pool: Arc<MemoryMediaPool>) -> Self {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let persistence = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.0.to_string_lossy().into_owned(),
                compression: JsonlCompression::None,
                ..Default::default()
            },
        )
        .unwrap();
        let storage = Storage::install(&ctx);
        let _ = storage
            .backend
            .register("memory", MemoryStorageBackend::with_shared_pool(pool))
            .unwrap();
        let _domains = DomainFacility::install(
            &ctx,
            DomainFacilityConfig {
                backend: "memory".into(),
                routes: Default::default(),
            },
        )
        .unwrap();
        let feedback = MessageFeedbackService::install(
            &ctx,
            &Config {
                max_note_bytes: 4096,
            },
        )
        .unwrap();
        Self {
            ctx,
            sessions,
            persistence,
            feedback,
        }
    }
    async fn create(&self, name: &str, created_at: u64) -> SessionHeader {
        let header = SessionHeader {
            id: session_id(name),
            created_at,
            cwd: Some(
                std::env::temp_dir()
                    .join("工作区")
                    .to_string_lossy()
                    .into_owned(),
            ),
            version: dsh_session::SESSION_FORMAT_VERSION,
            parent_session: None,
            is_seeded: false,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        self.persistence.create(header.clone(), None).await.unwrap();
        self.persistence
            .append(&header.id, &[assistant_event()])
            .await
            .unwrap();
        header
    }
    async fn list(&self, header: &SessionHeader) -> Vec<MessageFeedbackItem> {
        self.feedback
            .list(&MessageFeedbackListRequest {
                session_id: header.id.clone(),
            })
            .await
            .unwrap()
            .value
            .items
    }
    async fn close(self) {
        self.feedback.dispose();
        self.ctx.fiber.dispose().await;
    }
}
fn assistant_event() -> SessionEvent {
    SessionEvent {
        type_: "assistant/message".into(),
        seq: SessionSeq::new(0).unwrap(),
        time: 1,
        data: json!({"turn":1,"step":1,"message":{"id":"assistant-1","role":"assistant","content":[{"type":"text","text":"A finalized answer"}],"source":{"kind":"model","provider":"fixture","model":"fixture"}}}),
        ignorable: None,
        surface_op: Some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}
fn put(
    header: &SessionHeader,
    version: Option<MessageFeedbackVersion>,
    note: &str,
) -> MessageFeedbackPutRequest {
    MessageFeedbackPutRequest {
        session_id: header.id.clone(),
        message_id: dsh_llm::MessageId::new("assistant-1"),
        rating: MessageFeedbackRating::Negative,
        note: Some(note.into()),
        category: None,
        if_version: version,
    }
}

#[tokio::test]
async fn categories_survive_replay_and_participate_in_cas_without_requiring_a_note() {
    let root = TempRoot::new();
    let pool = Arc::new(MemoryMediaPool::new());
    let fixture = Fixture::new(&root, pool.clone());
    let header = fixture.create("categories", 1).await;
    let mut request = put(&header, None, "unused");
    request.note = None;
    request.rating = MessageFeedbackRating::Positive;
    request.category = Some("task-result".into());
    let first = fixture.feedback.put(&request).await.unwrap().value;
    assert_eq!(first.category.as_deref(), Some("task-result"));
    assert!(first.note.is_none());
    request.if_version = Some(first.version.clone());
    let same = fixture.feedback.put(&request).await.unwrap().value;
    assert_eq!(same.version, first.version);
    request.category = Some("instruction-following".into());
    let changed = fixture.feedback.put(&request).await.unwrap().value;
    assert_ne!(changed.version, first.version);
    assert!(matches!(
        fixture.feedback.put(&request).await.unwrap_err().error,
        MessageFeedbackFailure::VersionConflict { .. }
    ));
    request.if_version = Some(changed.version.clone());
    request.category = Some("unknown-category".into());
    let before = fixture
        .persistence
        .read_from(&header.id, 0)
        .await
        .unwrap()
        .events
        .len();
    assert!(matches!(
        fixture.feedback.put(&request).await.unwrap_err().error,
        MessageFeedbackFailure::CategoryInvalid
    ));
    assert_eq!(
        fixture
            .persistence
            .read_from(&header.id, 0)
            .await
            .unwrap()
            .events
            .len(),
        before
    );
    fixture.close().await;
    let restored = Fixture::new(&root, Arc::new(MemoryMediaPool::new()));
    assert_eq!(restored.list(&header).await, vec![changed.clone()]);
    request.category = None;
    let cleared = restored.feedback.put(&request).await.unwrap().value;
    assert!(cleared.category.is_none());
    restored
        .feedback
        .delete(&MessageFeedbackDeleteRequest {
            session_id: header.id.clone(),
            message_id: cleared.message_id.clone(),
            if_version: cleared.version,
        })
        .await
        .unwrap();
    assert!(restored.list(&header).await.is_empty());
    restored.close().await;
}

#[tokio::test]
async fn cold_feedback_survives_index_failure_restart_and_cas_races() {
    let root = TempRoot::new();
    let pool = Arc::new(MemoryMediaPool::new());
    let fixture = Fixture::new(&root, pool.clone());
    let header = fixture.create("cold", 1).await;
    fixture.close().await;
    let legacy = MessageFeedbackItem {
        message_id: dsh_llm::MessageId::new("assistant-1"),
        rating: MessageFeedbackRating::Positive,
        note: None,
        category: None,
        version: MessageFeedbackVersion::new(uuid::Uuid::new_v4().to_string()),
        created_at: 1,
        updated_at: 1,
    };
    pool.media
        .lock()
        .entry("message_feedback".into())
        .or_default()
        .tables
        .entry("sessions".into())
        .or_default()
        .insert(
            header.id.to_string(),
            serde_json::to_value(MessageFeedbackRow {
                session: MessageFeedbackSessionIdentity {
                    created_at: header.created_at,
                    cwd: header.cwd.clone(),
                },
                items: vec![legacy.clone()],
            })
            .unwrap(),
        );
    let fixture = Fixture::new(&root, pool.clone());
    assert_eq!(fixture.list(&header).await, vec![legacy.clone()]);
    pool.fail_next_writes.store(1, Ordering::SeqCst);
    let saved = fixture
        .feedback
        .put(&put(&header, Some(legacy.version), "correctness issue"))
        .await
        .unwrap()
        .value;
    assert!(
        fixture.sessions.list().is_empty(),
        "cold feedback must not create a live session or wake an Agent"
    );
    let log = fixture.persistence.read_from(&header.id, 0).await.unwrap();
    assert_eq!(log.events.len(), 2);
    assert_eq!(log.events[1].type_, "feedback/message-put");
    assert!(
        dsh_session::derive_event_message(&log.events[1]).is_none(),
        "feedback never enters model messages"
    );
    fixture.close().await;
    let fixture = Fixture::new(&root, pool.clone());
    assert_eq!(
        fixture.list(&header).await,
        vec![saved.clone()],
        "log overrides the stale legacy index after restart"
    );
    let left = put(&header, Some(saved.version.clone()), "left");
    let right = put(&header, Some(saved.version), "right");
    let (a, b) = tokio::join!(fixture.feedback.put(&left), fixture.feedback.put(&right));
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "only one concurrent CAS may replace a revision"
    );
    let current = fixture.list(&header).await.remove(0);
    let count = fixture
        .persistence
        .read_from(&header.id, 0)
        .await
        .unwrap()
        .events
        .len();
    fixture
        .feedback
        .put(&put(
            &header,
            Some(current.version.clone()),
            current.note.as_deref().unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .persistence
            .read_from(&header.id, 0)
            .await
            .unwrap()
            .events
            .len(),
        count,
        "same-value CAS does not append a duplicate event"
    );
    pool.fail_next_writes.store(1, Ordering::SeqCst);
    fixture
        .feedback
        .delete(&MessageFeedbackDeleteRequest {
            session_id: header.id.clone(),
            message_id: current.message_id,
            if_version: current.version,
        })
        .await
        .unwrap();
    fixture.close().await;
    let fixture = Fixture::new(&root, pool);
    assert!(
        fixture.list(&header).await.is_empty(),
        "durable deletion overrides a failed index write after restart"
    );
    fixture.persistence.load(&header.id).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn live_annotations_keep_sequence_ownership_and_stay_out_of_model_history() {
    let root = TempRoot::new();
    let fixture = Fixture::new(&root, Arc::new(MemoryMediaPool::new()));
    let header = fixture.create("live", 1).await;
    let preparation = fixture.persistence.prepare(&header.id).await.unwrap();
    let session = preparation.session.clone();
    let detach = fixture.sessions.enter(&session).unwrap();
    fixture.sessions.announce(&session).await.unwrap();
    drop(preparation);
    let before = session.derive_messages().unwrap().len();
    let request = put(&header, None, "issue");
    let feedback = fixture.feedback.put(&request);
    let title = async {
        session
            .append(
                "session/title",
                json!({"title":"Updated while rating"}),
                None,
            )
            .unwrap();
        fixture.sessions.flush(&session).await.unwrap();
    };
    let (result, ()) = tokio::join!(feedback, title);
    result.unwrap();
    assert_eq!(session.derive_messages().unwrap().len(), before);
    let persisted = fixture.persistence.read_from(&header.id, 0).await.unwrap();
    assert_eq!(persisted.events.len(), session.events().len());
    assert!(
        persisted
            .events
            .iter()
            .enumerate()
            .all(|(index, event)| event.seq.get() == index as u64)
    );
    assert_eq!(
        persisted
            .events
            .iter()
            .filter(|event| event.type_ == "feedback/message-put")
            .count(),
        1
    );
    assert!(
        !persisted
            .events
            .iter()
            .any(|event| event.type_ == "turn/start")
    );
    detach().await;
    fixture.close().await;
}

#[tokio::test]
async fn fork_and_reused_ids_cannot_inherit_another_lifecycles_feedback() {
    let root = TempRoot::new();
    let fixture = Fixture::new(&root, Arc::new(MemoryMediaPool::new()));
    let header = fixture.create("parent", 1).await;
    let item = fixture
        .feedback
        .put(&put(&header, None, "parent issue"))
        .await
        .unwrap()
        .value;
    let parent_log = fixture.persistence.read_from(&header.id, 0).await.unwrap();
    let child = SessionHeader {
        id: session_id("fork"),
        created_at: 2,
        parent_session: Some(header.id.clone()),
        is_seeded: true,
        ..header.clone()
    };
    fixture
        .persistence
        .create(
            child.clone(),
            Some(dsh_session::SessionLogOffset::new(parent_log.events.len() as u64).unwrap()),
        )
        .await
        .unwrap();
    fixture
        .persistence
        .append(&child.id, &parent_log.events)
        .await
        .unwrap();
    assert!(
        fixture.list(&child).await.is_empty(),
        "fork-inherited log entries cannot copy the parent's ratings"
    );
    fixture.persistence.delete(&header.id).await.unwrap();
    let replacement = fixture.create("parent", 100).await;
    assert!(
        fixture.list(&replacement).await.is_empty(),
        "legacy sidecar is fenced to the old lifecycle"
    );
    assert!(
        fixture
            .persistence
            .append_annotation(&header, "feedback/message-delete", json!({}))
            .await
            .is_err(),
        "a stale metadata capability cannot append to the new lifecycle"
    );
    let rejected = fixture
        .feedback
        .put(&put(&replacement, Some(item.version), "stale"))
        .await
        .unwrap_err();
    assert!(matches!(
        rejected.error,
        MessageFeedbackFailure::VersionConflict { current: None }
    ));
    fixture.close().await;
}
