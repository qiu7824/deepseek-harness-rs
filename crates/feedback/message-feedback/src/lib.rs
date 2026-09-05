//! Durable, lifecycle-bound feedback for finalized assistant messages.
//! Rust port of `packages/feedback/message-feedback/src/index.ts`
//! (+ `spec.ts`, `types.ts`).
//!
//! # Deviations
//!
//! - The storage-domain zod schemas become JSON validation closures (the
//!   established storage-domain collapse).
//! - The domain opens synchronously at install (the TS `Service.init`
//!   fiber step becomes a blocked open, matching the projection-cache
//!   pattern).
//! - The per-session operation queue is a `tokio::sync::Mutex` tail chain.

pub mod invariant;
mod recorded;
pub use recorded::NegativeFeedbackRecorded;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_brand::Branded;
use dsh_session::{SessionHeader, SessionId, derive_event_message, is_append_surface_event};
use dsh_session_persistence::{SessionInspection, SessionPersistenceApi};
use dsh_storage_domain::{Domain, DomainSpec, KvTable, define_domain, domain_table};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// The brand marker for [`MessageFeedbackVersion`].
#[doc(hidden)]
pub enum MessageFeedbackVersionTag {}

/// Opaque compare-and-set token for one exact feedback item revision.
pub type MessageFeedbackVersion = Branded<MessageFeedbackVersionTag>;

/// The human's overall judgment of one assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageFeedbackRating {
    Positive,
    Negative,
}

impl MessageFeedbackRating {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageFeedbackRating::Positive => "positive",
            MessageFeedbackRating::Negative => "negative",
        }
    }
}

/// One current feedback value and its opaque mutation token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackItem {
    pub message_id: dsh_llm::MessageId,
    pub rating: MessageFeedbackRating,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub version: MessageFeedbackVersion,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Persisted Session fields that fence a sidecar row to one log lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackSessionIdentity {
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One whole-Session sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageFeedbackRow {
    pub session: MessageFeedbackSessionIdentity,
    pub items: Vec<MessageFeedbackItem>,
}

/// The closed failure vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum MessageFeedbackFailure {
    SessionNotFound {
        session_id: SessionId,
    },
    TargetNotFound {
        session_id: SessionId,
        message_id: dsh_llm::MessageId,
    },
    VersionConflict {
        current: Option<MessageFeedbackItem>,
    },
    NoteBlank,
    NoteTooLarge {
        max_bytes: u64,
        actual_bytes: u64,
    },
}

/// Successful public operation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageFeedbackSuccess<T> {
    pub ok: bool,
    pub value: T,
}

/// Rejected public operation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageFeedbackRejected {
    pub ok: bool,
    pub error: MessageFeedbackFailure,
}

impl<T> MessageFeedbackSuccess<T> {
    fn of(value: T) -> Self {
        Self { ok: true, value }
    }
}

impl MessageFeedbackRejected {
    fn of(error: MessageFeedbackFailure) -> Self {
        Self { ok: false, error }
    }
}

/// Read request / value shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackListRequest {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageFeedbackListValue {
    pub items: Vec<MessageFeedbackItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackPutRequest {
    pub session_id: SessionId,
    pub message_id: dsh_llm::MessageId,
    pub rating: MessageFeedbackRating,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub if_version: Option<MessageFeedbackVersion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackDeleteRequest {
    pub session_id: SessionId,
    pub message_id: dsh_llm::MessageId,
    pub if_version: MessageFeedbackVersion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageFeedbackDeleteValue {
    pub absent: bool,
}

pub type MessageFeedbackListResult =
    Result<MessageFeedbackSuccess<MessageFeedbackListValue>, MessageFeedbackRejected>;
pub type MessageFeedbackPutResult =
    Result<MessageFeedbackSuccess<MessageFeedbackItem>, MessageFeedbackRejected>;
pub type MessageFeedbackDeleteResult =
    Result<MessageFeedbackSuccess<MessageFeedbackDeleteValue>, MessageFeedbackRejected>;

/// Required deployment policy for optional notes.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum UTF-8 byte length accepted for one note.
    pub max_note_bytes: u64,
}

/// Validate the one deployment-varying limit at the configuration boundary.
pub fn resolve_max_note_bytes(value: u64) -> Result<u64, String> {
    if value == 0 {
        return Err(format!(
            "message-feedback: maxNoteBytes must be a positive safe integer, got {value}"
        ));
    }
    Ok(value)
}

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Validate one stored item (the zod item schema translated).
fn validate_item(item: &JsonValue) -> Result<(), String> {
    let message_id = item
        .get("messageId")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "messageId must be a non-empty string".to_string())?;
    if message_id.is_empty() {
        return Err("messageId must be a non-empty string".to_string());
    }
    let rating = item
        .get("rating")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "rating must be positive or negative".to_string())?;
    if rating != "positive" && rating != "negative" {
        return Err("rating must be positive or negative".to_string());
    }
    if let Some(note) = item.get("note").and_then(|value| value.as_str())
        && note.trim().is_empty()
    {
        return Err("message feedback note must contain a non-whitespace character".to_string());
    }
    let version = item
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "version must be a uuid".to_string())?;
    uuid::Uuid::parse_str(version).map_err(|_| "version must be a uuid".to_string())?;
    for field in ["createdAt", "updatedAt"] {
        let value = item.get(field).and_then(|value| value.as_u64());
        if value.is_none_or(|value| value > MAX_SAFE_INTEGER) {
            return Err(format!("{field} must be a non-negative safe integer"));
        }
    }
    let created_at = item["createdAt"].as_u64().expect("checked");
    let updated_at = item["updatedAt"].as_u64().expect("checked");
    if updated_at < created_at {
        return Err("message feedback updatedAt must not precede createdAt".to_string());
    }
    Ok(())
}

/// Validate one stored row (the zod row schema translated).
pub fn validate_row(value: &JsonValue) -> Result<(), String> {
    let session = value
        .get("session")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "row.session must be an object".to_string())?;
    let created_at = session
        .get("createdAt")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "row.session.createdAt must be a non-negative safe integer".to_string())?;
    if created_at > MAX_SAFE_INTEGER {
        return Err("row.session.createdAt must be a non-negative safe integer".to_string());
    }
    if let Some(cwd) = session.get("cwd")
        && !cwd.is_string()
    {
        return Err("row.session.cwd must be a string".to_string());
    }
    let items = value
        .get("items")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "row.items must be an array".to_string())?;
    let mut message_ids = std::collections::HashSet::new();
    let mut versions = std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        validate_item(item)?;
        let message_id = item["messageId"].as_str().expect("checked").to_string();
        if !message_ids.insert(message_id.clone()) {
            return Err(format!("duplicate message feedback id '{message_id}'"));
        }
        let version = item["version"].as_str().expect("checked").to_string();
        if !versions.insert(version.clone()) {
            return Err(format!("duplicate message feedback version '{version}'"));
        }
        let _ = index;
    }
    Ok(())
}

/// The durable storage-domain declaration.
pub fn message_feedback_domain_spec() -> DomainSpec {
    define_domain(
        "message_feedback",
        0,
        None,
        indexmap::IndexMap::from([("sessions".to_string(), domain_table(Arc::new(validate_row)))]),
    )
    .expect("static spec")
}

fn identity_of(header: &SessionHeader) -> MessageFeedbackSessionIdentity {
    MessageFeedbackSessionIdentity {
        created_at: header.created_at,
        cwd: header.cwd.clone(),
    }
}

fn same_identity(row: &MessageFeedbackRow, header: &SessionHeader) -> bool {
    row.session.created_at == header.created_at && row.session.cwd == header.cwd
}

fn same_header_identity(left: &SessionHeader, right: &SessionHeader) -> bool {
    left.id == right.id && left.created_at == right.created_at && left.cwd == right.cwd
}

/// Storage-domain sidecar service (TS `MessageFeedbackService`).
pub struct MessageFeedbackService {
    ctx: Context,
    max_note_bytes: u64,
    table: Mutex<Option<Arc<dyn KvTable>>>,
    operation_tails: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    mutation_admission_open: AtomicBool,
    _domain: parking_lot::Mutex<Option<Arc<Domain>>>,
}

impl MessageFeedbackService {
    /// Open and own the sidecar domain, then publish the service (the TS
    /// constructor + `Service.init` collapse; the open is blocked like the
    /// projection-cache install).
    pub fn install(ctx: &Context, config: &Config) -> Result<Arc<Self>, String> {
        let max_note_bytes = resolve_max_note_bytes(config.max_note_bytes)?;
        let facility = ctx
            .get_typed::<Arc<dsh_storage_domain::DomainFacility>>("storageDomain", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "the storage domain facility is not configured".to_string())?;
        let domain = futures::executor::block_on(facility.open(&message_feedback_domain_spec()))?;
        let table = domain.table("sessions");
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            max_note_bytes,
            table: Mutex::new(Some(table)),
            operation_tails: Mutex::new(HashMap::new()),
            mutation_admission_open: AtomicBool::new(true),
            _domain: parking_lot::Mutex::new(Some(domain)),
        });
        ctx.register_service(service.clone());
        Ok(service)
    }

    /// Read feedback belonging to the current persisted Session lifecycle.
    pub async fn list(&self, request: &MessageFeedbackListRequest) -> MessageFeedbackListResult {
        let known = self.inspect_session(&request.session_id).await?;
        let row = self
            .table
            .lock()
            .as_ref()
            .expect("table")
            .get(request.session_id.as_str())
            .and_then(|value| serde_json::from_value::<MessageFeedbackRow>(value).ok());
        let items = match &row {
            Some(row) if same_identity(row, &known.meta) => row.items.clone(),
            _ => Vec::new(),
        };
        Ok(MessageFeedbackSuccess::of(MessageFeedbackListValue {
            items,
        }))
    }

    /// Create or replace feedback for one derived append-origin assistant
    /// message.
    pub async fn put(&self, request: &MessageFeedbackPutRequest) -> MessageFeedbackPutResult {
        let note = match self.resolve_note(request.note.as_deref()) {
            Ok(note) => note,
            Err(error) => return Err(MessageFeedbackRejected::of(error)),
        };
        self.enqueue(&request.session_id, async {
            let known = self.inspect_session(&request.session_id).await?;
            if !has_feedback_target(&known, &request.message_id) {
                return Err(MessageFeedbackRejected::of(
                    MessageFeedbackFailure::TargetNotFound {
                        session_id: request.session_id.clone(),
                        message_id: request.message_id.clone(),
                    },
                ));
            }
            let durable = self.ensure_target_durable(&known).await;
            if !same_header_identity(&durable.meta, &known.meta)
                || !has_feedback_target(&durable, &request.message_id)
            {
                return Err(MessageFeedbackRejected::of(
                    MessageFeedbackFailure::TargetNotFound {
                        session_id: request.session_id.clone(),
                        message_id: request.message_id.clone(),
                    },
                ));
            }
            let table = self.table.lock().as_ref().expect("table").clone();
            let stored = table
                .get(request.session_id.as_str())
                .and_then(|value| serde_json::from_value::<MessageFeedbackRow>(value).ok());
            let current = match &stored {
                Some(stored) if same_identity(stored, &durable.meta) => Some(stored),
                _ => None,
            };
            let mut items: Vec<MessageFeedbackItem> = current
                .map(|current| current.items.clone())
                .unwrap_or_default();
            let index = items
                .iter()
                .position(|item| item.message_id == request.message_id);
            let existing = index.map(|index| items[index].clone());
            if request.if_version != existing.as_ref().map(|item| item.version.clone()) {
                return Err(MessageFeedbackRejected::of(
                    MessageFeedbackFailure::VersionConflict { current: existing },
                ));
            }
            if let Some(existing) = &existing
                && existing.rating == request.rating
                && existing.note == note
            {
                return Ok(MessageFeedbackSuccess::of(existing.clone()));
            }
            let now = chrono::Utc::now().timestamp_millis() as u64;
            let item = MessageFeedbackItem {
                message_id: request.message_id.clone(),
                rating: request.rating,
                note: note.clone(),
                version: dsh_brand::Branded::new(uuid::Uuid::new_v4().to_string()),
                created_at: existing
                    .as_ref()
                    .map(|existing| existing.created_at)
                    .unwrap_or(now),
                updated_at: match &existing {
                    None => now,
                    Some(existing) => now.max(existing.updated_at),
                },
            };
            if let Some(index) = index {
                items[index] = item.clone();
            } else {
                items.push(item.clone());
            }
            let row = MessageFeedbackRow {
                session: identity_of(&durable.meta),
                items,
            };
            table
                .put(
                    request.session_id.as_str(),
                    serde_json::to_value(&row).expect("row"),
                )
                .await
                .expect("message-feedback: table.put failed");
            if let Some(observation) =
                recorded::negative_observation(&durable, &item, existing.as_ref())
            {
                self.ctx
                    .emit("message-feedback/recorded", vec![cordis::arc(observation)]);
            }
            Ok(MessageFeedbackSuccess::of(item))
        })
        .await
    }

    /// Delete one feedback item.
    pub async fn delete(
        &self,
        request: &MessageFeedbackDeleteRequest,
    ) -> MessageFeedbackDeleteResult {
        self.enqueue(&request.session_id, async {
            let known = self.inspect_session(&request.session_id).await?;
            let table = self.table.lock().as_ref().expect("table").clone();
            let stored = table
                .get(request.session_id.as_str())
                .and_then(|value| serde_json::from_value::<MessageFeedbackRow>(value).ok());
            let current = match &stored {
                Some(stored) if same_identity(stored, &known.meta) => Some(stored),
                _ => None,
            };
            let items: Vec<MessageFeedbackItem> = current
                .map(|current| current.items.clone())
                .unwrap_or_default();
            let existing = items
                .iter()
                .find(|item| item.message_id == request.message_id);
            let Some(existing) = existing else {
                return Ok(MessageFeedbackSuccess::of(MessageFeedbackDeleteValue {
                    absent: true,
                }));
            };
            if request.if_version != existing.version {
                return Err(MessageFeedbackRejected::of(
                    MessageFeedbackFailure::VersionConflict {
                        current: Some(existing.clone()),
                    },
                ));
            }
            let remaining: Vec<MessageFeedbackItem> = items
                .into_iter()
                .filter(|item| item.message_id != request.message_id)
                .collect();
            table
                .put(
                    request.session_id.as_str(),
                    serde_json::to_value(&MessageFeedbackRow {
                        session: identity_of(&known.meta),
                        items: remaining,
                    })
                    .expect("row"),
                )
                .await
                .expect("put");
            Ok(MessageFeedbackSuccess::of(MessageFeedbackDeleteValue {
                absent: true,
            }))
        })
        .await
    }

    async fn inspect_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionInspection, MessageFeedbackRejected> {
        let persistence = self
            .ctx
            .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
            .map(|slot| slot.as_ref().clone());
        let Some(persistence) = persistence else {
            return Err(MessageFeedbackRejected::of(
                MessageFeedbackFailure::SessionNotFound {
                    session_id: session_id.clone(),
                },
            ));
        };
        let live = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone());
        if live
            .as_ref()
            .is_none_or(|store| store.get(session_id).is_none())
        {
            let snapshots = persistence
                .list_snapshots()
                .await
                .expect("message-feedback: listSnapshots failed");
            if !snapshots
                .iter()
                .any(|snapshot| snapshot.header.id == *session_id)
                && live
                    .as_ref()
                    .is_none_or(|store| store.get(session_id).is_none())
            {
                return Err(MessageFeedbackRejected::of(
                    MessageFeedbackFailure::SessionNotFound {
                        session_id: session_id.clone(),
                    },
                ));
            }
        }
        persistence
            .inspect(session_id)
            .await
            .map_err(|error| panic!("message-feedback: inspect failed: {error}"))
    }

    async fn ensure_target_durable(&self, inspection: &SessionInspection) -> SessionInspection {
        let persistence = self
            .ctx
            .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
            .map(|slot| slot.as_ref().clone())
            .expect("message-feedback: no persistence service");
        let store = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone());
        let live = store
            .as_ref()
            .and_then(|store| store.get(&inspection.meta.id));
        if let Some(live) = &live
            && same_header_identity(live.header(), &inspection.meta)
        {
            let flushed = store
                .as_ref()
                .expect("store")
                .flush(live)
                .await
                .expect("message-feedback: session flush failed");
            if !flushed {
                panic!(
                    "message-feedback: no durability listener participated for live session '{}'",
                    inspection.meta.id
                );
            }
        }
        let result = persistence
            .read_from(&inspection.meta.id, 0)
            .await
            .expect("message-feedback: readFrom failed");
        SessionInspection {
            meta: result.meta,
            inherited_event_count: result.inherited_event_count,
            events: result.events,
        }
    }

    fn resolve_note(&self, note: Option<&str>) -> Result<Option<String>, MessageFeedbackFailure> {
        let Some(note) = note else {
            return Ok(None);
        };
        if note.trim().is_empty() {
            return Err(MessageFeedbackFailure::NoteBlank);
        }
        let actual_bytes = note.len() as u64;
        if actual_bytes > self.max_note_bytes {
            return Err(MessageFeedbackFailure::NoteTooLarge {
                max_bytes: self.max_note_bytes,
                actual_bytes,
            });
        }
        Ok(Some(note.to_string()))
    }

    /// Queue a complete read/compare/write mutation behind this Session's
    /// prior mutation.
    async fn enqueue<F, T>(
        &self,
        session_id: &SessionId,
        operation: F,
    ) -> Result<T, MessageFeedbackRejected>
    where
        F: std::future::Future<Output = Result<T, MessageFeedbackRejected>>,
    {
        if !self.mutation_admission_open.load(Ordering::SeqCst) {
            panic!("message-feedback: service is disposing");
        }
        let key = session_id.as_str().to_string();
        let tail = self
            .operation_tails
            .lock()
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = tail.lock().await;
        operation.await
    }

    /// Close the domain and reject new mutations (the TS effect cleanup;
    /// the close is blocked because the domain close future is not Send).
    pub fn dispose(&self) {
        self.mutation_admission_open.store(false, Ordering::SeqCst);
        let domain = self._domain.lock().take();
        self.table.lock().take();
        if let Some(domain) = domain {
            futures::executor::block_on(domain.close());
        }
    }
}

fn has_feedback_target(inspection: &SessionInspection, message_id: &dsh_llm::MessageId) -> bool {
    inspection.events.iter().any(|event| {
        if event.type_ != "assistant/message" || !is_append_surface_event(event) {
            return false;
        }
        let message = derive_event_message(event);
        message.is_some_and(|message| {
            message.role == dsh_llm::Role::Assistant && message.id == *message_id
        })
    })
}

impl cordis::Service for MessageFeedbackService {
    fn service_name(&self) -> &'static str {
        "messageFeedback"
    }
}

/// The Cordis plugin form (TS loader mounts the class with the config).
pub struct MessageFeedbackPlugin {
    config: Config,
}

impl MessageFeedbackPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for MessageFeedbackPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("message-feedback")
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["storageDomain", "sessionPersistence", "sessions"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let service = MessageFeedbackService::install(ctx, &self.config)
            .map_err(|error| PluginError::from(anyhow::anyhow!(error)))?;
        let _ = ctx.effect(
            "message-feedback",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let service = service.clone();
                    Box::pin(async move {
                        service.dispose();
                    })
                }))
            }),
        );
        Ok(())
    }
}

// Re-export the spec surface.
pub use dsh_storage_domain::DomainSpec as MessageFeedbackDomainSpecType;
