//! Rust port of the core `message-feedback.spec.ts` behaviors: lifecycle
//! fencing, version-gated put/delete, note validation, target resolution,
//! and the durable row schema — driven over an in-memory storage backend
//! with a fake persistence inspection.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use dsh_message_feedback::{
    Config, MessageFeedbackDeleteRequest, MessageFeedbackFailure, MessageFeedbackItem,
    MessageFeedbackListRequest, MessageFeedbackPutRequest, MessageFeedbackRating,
    MessageFeedbackService, validate_row,
};
use dsh_session::{SessionEvent, SessionHeader, SurfaceOp, assistant_message_data, session_id};
use dsh_session_persistence::{
    SessionInspection, SessionPersistenceApi, SessionPersistenceSnapshot,
    session_persistence_revision,
};
use dsh_storage::Storage;
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};

struct FakePersistence {
    inspections: HashMap<String, SessionInspection>,
}

impl FakePersistence {
    fn new(inspections: HashMap<String, SessionInspection>) -> Self {
        Self { inspections }
    }
}

#[async_trait::async_trait]
impl SessionPersistenceApi for FakePersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        None
    }
    fn supports_raw_artifacts(&self) -> bool {
        false
    }
    async fn create(&self, _meta: SessionHeader) -> Result<(), String> {
        Ok(())
    }
    async fn append(
        &self,
        _id: &dsh_session::SessionId,
        _events: &[SessionEvent],
    ) -> Result<(), String> {
        Ok(())
    }
    async fn load(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        self.inspections
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| "missing".to_string())
    }
    async fn inspect(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        self.load(id).await
    }
    async fn read_from(
        &self,
        id: &dsh_session::SessionId,
        _from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        let inspection = self.load(id).await?;
        Ok(dsh_session_persistence::SessionReadFromResult {
            meta: inspection.meta,
            events: inspection.events,
        })
    }
    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self
            .inspections
            .values()
            .map(|inspection| inspection.meta.clone())
            .collect())
    }
    async fn list_snapshots(
        &self,
    ) -> Result<Vec<dsh_session_persistence::SessionPersistenceSnapshot>, String> {
        Ok(self
            .inspections
            .values()
            .map(|inspection| SessionPersistenceSnapshot {
                header: inspection.meta.clone(),
                revision: session_persistence_revision("r1"),
            })
            .collect())
    }
    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }
}

fn header_of(id: &str, created_at: u64, cwd: Option<&str>) -> SessionHeader {
    SessionHeader {
        version: 0,
        id: session_id(id),
        created_at,
        cwd: cwd.map(str::to_string),
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}

/// An append-origin assistant message event with a stable message id.
fn assistant_event(message_id: &str) -> SessionEvent {
    let message = dsh_llm::create_assistant_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "hello".to_string(),
        }],
        dsh_llm::ModelMessageSource {
            provider: "stub".to_string(),
            model: "stub".to_string(),
            replay_state: None,
        },
    );
    let mut message = message;
    message.id = dsh_llm::MessageId::new(message_id);
    SessionEvent {
        type_: "assistant/message".to_string(),
        seq: 0,
        time: 0,
        data: assistant_message_data(1, 1, &message, None),
        ignorable: None,
        surface_op: Some(SurfaceOp::Append),
        source_event_seqs: None,
    }
}

fn harness(
    inspections: HashMap<String, SessionInspection>,
) -> (Context, Arc<MessageFeedbackService>) {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = MemoryStorageBackend::with_shared_pool(Arc::new(MemoryMediaPool::new()));
    hub.backend
        .register("memory", backend)
        .expect("register backend");
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig {
            backend: "memory".to_string(),
            routes: Default::default(),
        },
    )
    .expect("facility");
    let _ = facility;
    let _store = dsh_session::SessionStore::install(&ctx);
    let persistence: Arc<dyn SessionPersistenceApi> = Arc::new(FakePersistence::new(inspections));
    ctx.register_service(persistence);
    let service =
        MessageFeedbackService::install(&ctx, &Config { max_note_bytes: 64 }).expect("install");
    (ctx, service)
}

fn inspection_for(id: &str, message_id: Option<&str>) -> SessionInspection {
    let mut events = Vec::new();
    if let Some(message_id) = message_id {
        events.push(assistant_event(message_id));
    }
    SessionInspection {
        meta: header_of(id, 100, Some("D:\\work")),
        events,
    }
}

#[test]
fn row_schema_rejects_invalid_shapes() {
    let good = serde_json::json!({
        "session": { "createdAt": 100, "cwd": "D:\\work" },
        "items": [{
            "messageId": "m1",
            "rating": "positive",
            "version": uuid::Uuid::new_v4().to_string(),
            "createdAt": 1,
            "updatedAt": 2,
        }]
    });
    validate_row(&good).expect("valid row");
    let dup = serde_json::json!({
        "session": { "createdAt": 100 },
        "items": [
            { "messageId": "m1", "rating": "positive", "version": "v", "createdAt": 1, "updatedAt": 2 },
            { "messageId": "m1", "rating": "negative", "version": "w", "createdAt": 1, "updatedAt": 2 }
        ]
    });
    assert!(validate_row(&dup).is_err());
    let bad_order = serde_json::json!({
        "session": { "createdAt": 100 },
        "items": [{ "messageId": "m1", "rating": "positive", "version": "v", "createdAt": 5, "updatedAt": 2 }]
    });
    assert!(validate_row(&bad_order).is_err());
    let blank_note = serde_json::json!({
        "session": { "createdAt": 100 },
        "items": [{ "messageId": "m1", "rating": "positive", "note": "  ", "version": "v", "createdAt": 1, "updatedAt": 2 }]
    });
    assert!(validate_row(&blank_note).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn lists_puts_and_deletes_with_version_gating() {
    let (ctx, service) = harness(HashMap::from([(
        "feedback-session".to_string(),
        inspection_for("feedback-session", Some("m-target")),
    )]));
    let session = session_id("feedback-session");

    // Empty list first.
    let listed = service
        .list(&MessageFeedbackListRequest {
            session_id: session.clone(),
        })
        .await
        .expect("list");
    assert!(listed.value.items.is_empty());

    // Create.
    let put = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            rating: MessageFeedbackRating::Positive,
            note: Some("great answer".to_string()),
            if_version: None,
        })
        .await
        .expect("put");
    let first_version = put.value.version.clone();
    assert_eq!(put.value.note.as_deref(), Some("great answer"));
    assert!(put.value.created_at <= put.value.updated_at);

    // List reflects it.
    let listed = service
        .list(&MessageFeedbackListRequest {
            session_id: session.clone(),
        })
        .await
        .expect("list");
    assert_eq!(listed.value.items.len(), 1);
    assert_eq!(listed.value.items[0].message_id.as_str(), "m-target");

    // A matching no-op put returns the same version.
    let noop = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            rating: MessageFeedbackRating::Positive,
            note: Some("great answer".to_string()),
            if_version: Some(first_version.clone()),
        })
        .await
        .expect("no-op put");
    assert_eq!(noop.value.version, first_version);

    // A stale version conflicts with the authoritative item.
    let stale = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            rating: MessageFeedbackRating::Negative,
            note: None,
            if_version: Some(dsh_brand::Branded::new("stale".to_string())),
        })
        .await;
    match stale {
        Err(rejected) => match rejected.error {
            MessageFeedbackFailure::VersionConflict { current } => {
                assert!(current.is_some());
            }
            other => panic!("version-conflict expected, got {other:?}"),
        },
        Ok(_) => panic!("conflict expected"),
    }

    // An unknown target rejects.
    let missing = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("no-such"),
            rating: MessageFeedbackRating::Positive,
            note: None,
            if_version: None,
        })
        .await;
    assert!(matches!(
        missing,
        Err(dsh_message_feedback::MessageFeedbackRejected {
            error: MessageFeedbackFailure::TargetNotFound { .. },
            ..
        })
    ));

    // Note validation.
    let blank = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            rating: MessageFeedbackRating::Positive,
            note: Some("   ".to_string()),
            if_version: Some(first_version.clone()),
        })
        .await;
    assert!(matches!(
        blank,
        Err(dsh_message_feedback::MessageFeedbackRejected {
            error: MessageFeedbackFailure::NoteBlank,
            ..
        })
    ));
    let too_large = service
        .put(&MessageFeedbackPutRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            rating: MessageFeedbackRating::Positive,
            note: Some("x".repeat(65)),
            if_version: Some(first_version.clone()),
        })
        .await;
    assert!(matches!(
        too_large,
        Err(dsh_message_feedback::MessageFeedbackRejected {
            error: MessageFeedbackFailure::NoteTooLarge { .. },
            ..
        })
    ));

    // Delete with a wrong version conflicts; the right version removes it.
    let conflict = service
        .delete(&MessageFeedbackDeleteRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            if_version: dsh_brand::Branded::new("stale".to_string()),
        })
        .await;
    assert!(matches!(
        conflict,
        Err(dsh_message_feedback::MessageFeedbackRejected {
            error: MessageFeedbackFailure::VersionConflict { .. },
            ..
        })
    ));
    let deleted = service
        .delete(&MessageFeedbackDeleteRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            if_version: first_version.clone(),
        })
        .await
        .expect("delete");
    assert!(deleted.value.absent);
    let listed = service
        .list(&MessageFeedbackListRequest {
            session_id: session.clone(),
        })
        .await
        .expect("list");
    assert!(listed.value.items.is_empty());

    // Deleting an already-absent item succeeds regardless of version.
    let retry = service
        .delete(&MessageFeedbackDeleteRequest {
            session_id: session.clone(),
            message_id: dsh_llm::MessageId::new("m-target"),
            if_version: first_version,
        })
        .await
        .expect("retry");
    assert!(retry.value.absent);
    let _ = ctx;
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_sessions_reject_as_session_not_found() {
    let (_ctx, service) = harness(HashMap::new());
    let listed = service
        .list(&MessageFeedbackListRequest {
            session_id: session_id("nobody"),
        })
        .await;
    assert!(matches!(
        listed,
        Err(dsh_message_feedback::MessageFeedbackRejected {
            error: MessageFeedbackFailure::SessionNotFound { .. },
            ..
        })
    ));
}

#[test]
fn item_wire_shape_round_trips() {
    let item = MessageFeedbackItem {
        message_id: dsh_llm::MessageId::new("m1"),
        rating: MessageFeedbackRating::Negative,
        note: Some("slow".to_string()),
        version: dsh_brand::Branded::new(uuid::Uuid::new_v4().to_string()),
        created_at: 1,
        updated_at: 2,
    };
    let json = serde_json::to_value(&item).expect("serialize");
    assert_eq!(json["messageId"], "m1");
    assert_eq!(json["rating"], "negative");
    let back: MessageFeedbackItem = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, item);
}
