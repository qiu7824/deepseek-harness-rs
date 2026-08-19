//! Rust port of the core `packages/subagent/subagent/tests/list-children.spec.ts`
//! + projection behaviors: the timing and identity projection units, and the
//!   live/cold child-listing ladder.
//!
//! # Deviations
//!
//! - The projection-cache rung falls through to the authoritative re-fold
//!   when a cached value is absent or below the seq gate (same contract,
//!   cache exercised via the registry watermark only).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, Session, SessionEvent, SessionHeader, SessionStore,
    session_id,
};
use dsh_session_persistence::{
    SessionInspection, SessionPersistenceApi, SessionPersistenceSnapshot,
    session_persistence_revision,
};
use dsh_session_projection::SessionProjectionRegistry;
use dsh_subagent::{
    SubagentListEntry, SubagentRuntime, SubagentTimingProjection,
    subagent_identity_projection_definition, subagent_timing_projection_definition,
};

fn descriptor_event(seq: u64, mode: &str, label: Option<&str>) -> SessionEvent {
    let schedule = match mode {
        "continuable" => serde_json::json!({
            "version": 2, "mode": "continuable", "provider": "fork", "label": label.unwrap_or("label")
        }),
        _ => serde_json::json!({
            "version": 2, "mode": "one-shot", "provider": "fork"
        }),
    };
    SessionEvent {
        type_: "subagent/descriptor".to_string(),
        seq,
        time: 1,
        data: schedule,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn turn_event(type_: &str, seq: u64, time: i64) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq,
        time,
        data: serde_json::json!({ "turn": 1 }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

async fn setup() -> Context {
    let ctx = Context::root();
    let _store = SessionStore::install(&ctx);
    let projections = SessionProjectionRegistry::install(&ctx);
    projections
        .register(&ctx, subagent_timing_projection_definition())
        .expect("timing");
    projections
        .register(&ctx, subagent_identity_projection_definition())
        .expect("identity");
    let _subagents = SubagentRuntime::install(&ctx);
    ctx
}

async fn child_session(
    ctx: &Context,
    store: &Arc<SessionStore>,
    id: &str,
    parent: Option<&str>,
    events: Vec<SessionEvent>,
    created_at: u64,
) -> Session {
    store
        .create(
            ctx,
            Some(session_id(id)),
            Some(CreateSessionOptions {
                seed: Some(events),
                meta: Some(CreateSessionMeta {
                    created_at: Some(created_at),
                    parent_session: parent.map(session_id),
                    origin: Some("subagent".to_string()),
                    seed_length: Some(0),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("session")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folds_timing_across_descriptor_and_turn_boundaries() {
    let ctx = setup().await;
    let projections = ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .map(|slot| slot.as_ref().clone())
        .expect("projections");
    let store = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessions");
    let session = child_session(
        &ctx,
        &store,
        "timed",
        None,
        vec![
            turn_event("turn/start", 0, 100),
            descriptor_event(1, "one-shot", None),
            turn_event("turn/end", 2, 200),
            turn_event("turn/start", 3, 300),
        ],
        1,
    )
    .await;
    let snapshot = projections.snapshot(&session);
    let timing: SubagentTimingProjection = serde_json::from_value(
        snapshot
            .values
            .get("subagentTiming")
            .expect("timing")
            .clone(),
    )
    .expect("timing json");
    // Seed replay may re-stamp event times; derive the expectation from the
    // authoritative log the projection folded.
    let events = session.events();
    let start_time = events
        .iter()
        .find(|event| event.type_ == "turn/start")
        .map(|event| event.time)
        .expect("start");
    let end_time = events
        .iter()
        .find(|event| event.type_ == "turn/end")
        .map(|event| event.time)
        .expect("end");
    let last_start = events
        .iter()
        .rev()
        .find(|event| event.type_ == "turn/start")
        .map(|event| event.time)
        .expect("last start");
    assert_eq!(timing.settled_ms, (end_time - start_time).max(0) as u64);
    let active = timing.active.expect("open turn");
    assert_eq!(active.since, last_start);
    // The final auto end-seed event also folds into the active cut (TS
    // parity): through equals the last event's time.
    let last_time = events.last().map(|event| event.time).expect("last");
    assert_eq!(active.through, last_time);
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_live_children_by_projection_identity() {
    let ctx = setup().await;
    let store = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessions");
    let runtime = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .expect("subagents");
    child_session(
        &ctx,
        &store,
        "child-one",
        Some("parent"),
        vec![descriptor_event(0, "continuable", Some("check build"))],
        10,
    )
    .await;
    child_session(
        &ctx,
        &store,
        "child-two",
        Some("parent"),
        vec![descriptor_event(0, "one-shot", None)],
        5,
    )
    .await;
    // A creation-window child without a descriptor is omitted.
    child_session(&ctx, &store, "child-early", Some("parent"), vec![], 3).await;
    // An ordinary session is never interpreted.
    store
        .create(
            &ctx,
            Some(session_id("ordinary")),
            Some(CreateSessionOptions {
                seed: None,
                meta: Some(CreateSessionMeta {
                    created_at: Some(1),
                    parent_session: Some(session_id("parent")),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect("ordinary");

    let children = runtime
        .list_children(&session_id("parent"), None)
        .await
        .expect("children");
    // Ordered by createdAt: child-two (5), child-one (10).
    let ids: Vec<String> = children
        .iter()
        .map(|entry| match entry {
            SubagentListEntry::Child { id, .. } => id.as_str().to_string(),
            SubagentListEntry::Diagnostic { id, .. } => id.as_str().to_string(),
        })
        .collect();
    assert_eq!(ids, vec!["child-two", "child-one"]);
    assert!(matches!(
        &children[0],
        SubagentListEntry::Child { activity, .. } if activity == "running"
    ));
    assert!(matches!(
        &children[1],
        SubagentListEntry::Child {
            identity: dsh_subagent::SubagentIdentityProjection::Continuable { label, .. },
            ..
        } if label == "check build"
    ));
}

/// A cold-store fake persistence backend.
struct ColdPersistence {
    entries: HashMap<String, (SessionHeader, Vec<SessionEvent>)>,
}

#[async_trait::async_trait]
impl SessionPersistenceApi for ColdPersistence {
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

    async fn inspect(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        let Some((meta, events)) = self.entries.get(id.as_str()) else {
            return Err("missing".to_string());
        };
        Ok(SessionInspection {
            meta: meta.clone(),
            events: events.clone(),
        })
    }

    async fn load(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        self.inspect(id).await
    }

    async fn read_from(
        &self,
        id: &dsh_session::SessionId,
        from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        let whole = self.inspect(id).await?;
        Ok(dsh_session_persistence::SessionReadFromResult {
            meta: whole.meta,
            events: whole
                .events
                .into_iter()
                .filter(|event| event.seq >= from_seq)
                .collect(),
        })
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self
            .entries
            .values()
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String> {
        Ok(self
            .entries
            .iter()
            .map(|(id, (meta, _))| SessionPersistenceSnapshot {
                header: meta.clone(),
                revision: session_persistence_revision(format!("test:{id}")),
            })
            .collect())
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_cold_children_through_persistence_inspection() {
    let ctx = setup().await;
    let store = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .map(|slot| slot.as_ref().clone())
        .expect("sessions");
    let runtime = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .map(|slot| slot.as_ref().clone())
        .expect("subagents");
    // The parent lives; one child is cold in persistence, one corrupt.
    let parent = store
        .create(
            &ctx,
            Some(session_id("parent")),
            Some(CreateSessionOptions::default()),
        )
        .await
        .expect("parent");
    drop(parent);
    let cold_header = SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id("cold-child"),
        created_at: 7,
        cwd: None,
        parent_session: Some(session_id("parent")),
        seed_length: Some(0),
        origin: Some("subagent".to_string()),
        delegation_depth: None,
        agent_preset: None,
    };
    let corrupt_header = SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id("corrupt-child"),
        created_at: 8,
        cwd: None,
        parent_session: Some(session_id("parent")),
        seed_length: Some(0),
        origin: Some("subagent".to_string()),
        delegation_depth: None,
        agent_preset: None,
    };
    let persistence = ColdPersistence {
        entries: HashMap::from([
            (
                "cold-child".to_string(),
                (
                    cold_header.clone(),
                    vec![descriptor_event(0, "one-shot", None)],
                ),
            ),
            // A malformed current-version descriptor folds to no identity.
            (
                "corrupt-child".to_string(),
                (
                    corrupt_header.clone(),
                    vec![SessionEvent {
                        type_: "subagent/descriptor".to_string(),
                        seq: 0,
                        time: 1,
                        data: serde_json::json!({ "version": 2, "mode": "one-shot" }),
                        ignorable: None,
                        surface_op: None,
                        source_event_seqs: None,
                    }],
                ),
            ),
        ]),
    };
    let erased: Arc<dyn SessionPersistenceApi> = Arc::new(persistence);
    ctx.register_service(erased);

    let children = runtime
        .list_children(&session_id("parent"), None)
        .await
        .expect("children");
    let cold = children
        .iter()
        .find(|entry| match entry {
            SubagentListEntry::Child { id, .. } => id.as_str() == "cold-child",
            SubagentListEntry::Diagnostic { id, .. } => id.as_str() == "cold-child",
        })
        .expect("cold");
    assert!(matches!(
        cold,
        SubagentListEntry::Child { activity, .. } if activity == "inactive"
    ));
    let corrupt = children
        .iter()
        .find(|entry| match entry {
            SubagentListEntry::Diagnostic { id, .. } => id.as_str() == "corrupt-child",
            SubagentListEntry::Child { .. } => false,
        })
        .expect("corrupt");
    assert!(matches!(
        corrupt,
        SubagentListEntry::Diagnostic { reason, .. } if reason == "corrupt"
    ));
    let _ = store;
}
