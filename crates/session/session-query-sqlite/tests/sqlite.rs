//! Rust port of the core `packages/session-query/session-query-sqlite/tests/sqlite.spec.ts`
//! behaviors: opening modes, live-only FTS5 search, ranking, cursors, dynamic
//! persistence reconciliation, and schema lifecycle.
//!
//! # Deviations
//!
//! - The real SQLite persistence backend registers its concrete type, while
//!   the search backend observes the erased `Arc<dyn SessionPersistenceApi>`
//!   registration; the combined keyless integration is exercised through the
//!   [`TestPersistence`] fake, which registers erased.
//! - Test working directories use Windows absolute paths because the Rust
//!   session header validates path absoluteness on the host platform.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{Context, Disposer};
use dsh_session::{
    CreateSessionMeta, CreateSessionOptions, Session, SessionEvent, SessionHeader, SessionStore,
    SurfaceIntent, SurfaceOp, session_id,
};
use dsh_session_persistence::{
    SessionInspection, SessionPersistenceApi, SessionPersistenceSnapshot,
    session_persistence_revision,
};
use dsh_session_query::{
    SessionAvailability, SessionEventResultFilter, SessionEventSearchRequest, SessionEventSurface,
    SessionQueryEngine, SessionQueryErrorCode, SessionResultFilter, SessionSearchRequest,
};
use dsh_session_query_sqlite::{Config, OpenAt, SqliteSearch};

fn header(id: &str, created_at: u64, extra: CreateSessionMeta) -> SessionHeader {
    SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id(id),
        created_at,
        cwd: extra.cwd,
        parent_session: extra.parent_session,
        seed_length: extra.seed_length,
        origin: extra.origin,
        delegation_depth: extra.delegation_depth,
        agent_preset: extra.agent_preset,
    }
}

fn user_event(text: &str, seq: u64, time: i64, surface: SurfaceOp) -> SessionEvent {
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: text.to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    let data = serde_json::to_value(&message).expect("message");
    SessionEvent {
        type_: "user/message".to_string(),
        seq,
        time,
        data,
        ignorable: None,
        surface_op: Some(surface),
        source_event_seqs: None,
    }
}

fn message_events(text: &str, time: i64) -> Vec<SessionEvent> {
    vec![user_event(text, 0, time, SurfaceOp::Append)]
}

fn assistant_chunk_event(seq: u64, time: i64, text: &str) -> SessionEvent {
    SessionEvent {
        type_: "assistant/chunk".to_string(),
        seq,
        time,
        data: serde_json::json!({
            "turn": 1, "step": 1,
            "chunk": { "type": "text-delta", "index": 0, "text": text },
        }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn turn_end_event(seq: u64, time: i64, message: &str) -> SessionEvent {
    SessionEvent {
        type_: "turn/end".to_string(),
        seq,
        time,
        data: serde_json::json!({
            "turn": 1,
            "reason": { "kind": "error", "error": { "message": message, "code": "UNKNOWN" } },
        }),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn code(error: &dsh_session_query::SessionQueryError) -> SessionQueryErrorCode {
    error.code
}

fn live_context(
    config: Config,
) -> (
    Context,
    Arc<SqliteSearch>,
    Arc<SessionQueryEngine>,
    Arc<SessionStore>,
) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let search = SqliteSearch::install(&ctx, &config).expect("search install");
    let engine = ctx
        .get_typed::<Arc<SessionQueryEngine>>("sessionQuery", false)
        .map(|slot| slot.as_ref().clone())
        .expect("engine");
    (ctx, search, engine, store)
}

async fn create_session(
    store: &SessionStore,
    ctx: &Context,
    id: &str,
    events: Vec<SessionEvent>,
    created_at: u64,
    meta: CreateSessionMeta,
) -> Session {
    store
        .create(
            ctx,
            Some(session_id(id)),
            Some(CreateSessionOptions {
                seed: Some(events),
                meta: Some(CreateSessionMeta {
                    created_at: Some(created_at),
                    ..meta
                }),
            }),
        )
        .await
        .expect("session")
}

/// The TS `TestPersistence` fake: an in-memory backend with revision tokens,
/// inspection counters, and hookable gates/effects.
struct TestState {
    entries: HashMap<String, (SessionHeader, Vec<SessionEvent>)>,
    revisions: HashMap<String, u64>,
    next_revision: u64,
    inspections: HashMap<String, u64>,
    list_gate: Option<Arc<tokio::sync::Notify>>,
    failure: Option<String>,
    snapshot_effect: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct TestPersistence {
    ctx: Context,
    state: Arc<std::sync::Mutex<TestState>>,
}

impl TestPersistence {
    fn new(ctx: &Context) -> Arc<Self> {
        Arc::new(Self {
            ctx: ctx.clone(),
            state: Arc::new(std::sync::Mutex::new(TestState {
                entries: HashMap::new(),
                revisions: HashMap::new(),
                next_revision: 0,
                inspections: HashMap::new(),
                list_gate: None,
                failure: None,
                snapshot_effect: None,
            })),
        })
    }

    fn set(&self, meta: SessionHeader, events: Vec<SessionEvent>) {
        let mut state = self.state.lock().expect("state");
        state.next_revision += 1;
        let revision = state.next_revision;
        state
            .revisions
            .insert(meta.id.as_str().to_string(), revision);
        state
            .entries
            .insert(meta.id.as_str().to_string(), (meta, events));
    }

    fn reset(&self) {
        *self.state.lock().expect("state") = TestState {
            entries: HashMap::new(),
            revisions: HashMap::new(),
            next_revision: 0,
            inspections: HashMap::new(),
            list_gate: None,
            failure: None,
            snapshot_effect: None,
        };
    }

    fn inspections(&self, id: &str) -> u64 {
        *self
            .state
            .lock()
            .expect("state")
            .inspections
            .get(id)
            .unwrap_or(&0)
    }

    fn set_snapshot_effect(&self, effect: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.state.lock().expect("state").snapshot_effect = effect;
    }
}

#[async_trait::async_trait]
impl SessionPersistenceApi for TestPersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, meta: SessionHeader) -> Result<(), String> {
        self.set(meta, Vec::new());
        Ok(())
    }

    async fn append(
        &self,
        id: &dsh_session::SessionId,
        events: &[SessionEvent],
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("state");
        let Some((_, entry_events)) = state.entries.get_mut(id.as_str()) else {
            return Err("missing test session".to_string());
        };
        entry_events.extend(events.iter().cloned());
        state.next_revision += 1;
        let revision = state.next_revision;
        state
            .revisions
            .insert(id.as_str().to_string(), revision);
        Ok(())
    }

    async fn inspect(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        let failure = self.state.lock().expect("state").failure.clone();
        if let Some(failure) = failure {
            return Err(failure);
        }
        let mut state = self.state.lock().expect("state");
        *state.inspections.entry(id.as_str().to_string()).or_insert(0) += 1;
        let Some((meta, events)) = state.entries.get(id.as_str()) else {
            return Err("missing test session".to_string());
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
        let failure = self.state.lock().expect("state").failure.clone();
        if let Some(failure) = failure {
            return Err(failure);
        }
        Ok(self
            .state
            .lock()
            .expect("state")
            .entries
            .values()
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String> {
        let gate = self.state.lock().expect("state").list_gate.clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let failure = self.state.lock().expect("state").failure.clone();
        if let Some(failure) = failure {
            return Err(failure);
        }
        let snapshots = {
            let state = self.state.lock().expect("state");
            state
                .entries
                .iter()
                .map(|(id, (meta, _))| SessionPersistenceSnapshot {
                    header: meta.clone(),
                    revision: session_persistence_revision(format!(
                        "test:{}",
                        state.revisions.get(id).copied().unwrap_or(0)
                    )),
                })
                .collect::<Vec<_>>()
        };
        let effect = self.state.lock().expect("state").snapshot_effect.clone();
        if let Some(effect) = effect {
            effect();
        }
        Ok(snapshots)
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }
}

fn mount_persistence(ctx: &Context, persistence: &Arc<TestPersistence>) -> Disposer {
    let erased: Arc<dyn SessionPersistenceApi> = persistence.clone();
    ctx.register_service(erased)
}

async fn unmount(disposer: Disposer) {
    disposer().await;
}

#[tokio::test]
async fn opens_once_on_the_first_search_and_reuses_readiness() {
    let (_ctx, _search, engine, _store) = live_context(Config {
        path: ":memory:".to_string(),
        open_at: Some(OpenAt::FirstSearch),
        ..Default::default()
    });
    let first = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "first".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("first");
    assert!(first.items.is_empty());
    let second = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "second".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("second");
    assert!(second.items.is_empty());
}

#[tokio::test]
async fn refuses_search_in_never_mode_while_inherited_reads_keep_working() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        open_at: Some(OpenAt::Never),
        ..Default::default()
    });
    create_session(
        &store,
        &ctx,
        "never-parent",
        message_events("never opened needle", 1),
        10,
        CreateSessionMeta::default(),
    )
    .await;
    create_session(
        &store,
        &ctx,
        "never-child",
        vec![],
        20,
        CreateSessionMeta {
            parent_session: Some(session_id("never-parent")),
            ..Default::default()
        },
    )
    .await;

    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("disabled");
    assert_eq!(
        code(&error),
        SessionQueryErrorCode::SessionQuerySearchDisabled
    );
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("never-parent")),
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("disabled");
    assert_eq!(
        code(&error),
        SessionQueryErrorCode::SessionQuerySearchDisabled
    );

    let sessions = engine.list_sessions(None).await.expect("list");
    let mut ids: Vec<String> = sessions
        .iter()
        .map(|record| record.header.id.as_str().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["never-child", "never-parent"]);
}

#[tokio::test]
async fn searches_two_character_tokens_in_live_only_sessions() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        snippet_chars: Some(20),
        ..Default::default()
    });
    let session = create_session(
        &store,
        &ctx,
        "live",
        message_events("An AI helper", 1),
        10,
        CreateSessionMeta {
            cwd: Some("C:\\work".to_string()),
            seed_length: Some(1),
            delegation_depth: Some(2),
            agent_preset: Some("minimal".to_string()),
            ..Default::default()
        },
    )
    .await;

    let events = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session.id().clone()),
                query: "AI".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("events");
    assert_eq!(events.session.cwd.as_deref(), Some("C:\\work"));
    assert_eq!(events.session.seed_length, Some(1));
    assert_eq!(events.items.len(), 1);
    assert_eq!(events.items[0].session_id, session.id().clone());
    assert_eq!(events.items[0].seq, 0u64);
    assert_eq!(events.items[0].snippet, "An AI helper");

    let sessions = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "AI".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("sessions");
    assert_eq!(sessions.items.len(), 1);
    assert!(sessions.items[0].record.live);
    assert!(!sessions.items[0].record.persisted);
    assert_eq!(sessions.items[0].record.header.seed_length, Some(1));
}

#[tokio::test]
async fn searches_all_surfaces_and_applies_metadata_before_ranking() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        default_limit: Some(10),
        max_limit: Some(20),
        ..Default::default()
    });
    let events = vec![
        user_event("needle original", 0, 10, SurfaceOp::Append),
        assistant_chunk_event(1, 11, "needle raw"),
        SessionEvent {
            source_event_seqs: Some(vec![0]),
            ..user_event("needle summary", 2, 12, SurfaceOp::Replace { start: 0, end: 0 })
        },
        turn_end_event(3, 13, "needle failure"),
    ];
    create_session(
        &store,
        &ctx,
        "a",
        events,
        20,
        CreateSessionMeta {
            cwd: Some("C:\\a".to_string()),
            parent_session: Some(session_id("parent")),
            ..Default::default()
        },
    )
    .await;
    create_session(
        &store,
        &ctx,
        "b",
        message_events("needle peer", 12),
        20,
        CreateSessionMeta::default(),
    )
    .await;

    let all = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("a")),
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("all");
    let surfaces: Vec<SessionEventSurface> = all.items.iter().map(|item| item.surface).collect();
    assert!(surfaces.contains(&SessionEventSurface::Current));
    assert!(surfaces.contains(&SessionEventSurface::Shadowed));
    assert!(surfaces.contains(&SessionEventSurface::LogOnly));

    let filtered = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("a")),
                query: "needle".to_string(),
                filters: Some(vec![
                    SessionEventResultFilter::Seq {
                        from: Some(2.0),
                        to: Some(2.0),
                    },
                    SessionEventResultFilter::Time {
                        from: Some(12.0),
                        to: Some(12.0),
                    },
                    SessionEventResultFilter::Type {
                        values: vec!["user/message".to_string()],
                    },
                    SessionEventResultFilter::Surface {
                        values: vec![SessionEventSurface::Current],
                    },
                ]),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("filtered");
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].seq, 2u64);
    assert_eq!(filtered.items[0].surface, SessionEventSurface::Current);

    let grouped = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                session_filters: Some(vec![
                    SessionResultFilter::Id {
                        values: vec![session_id("a")],
                    },
                    SessionResultFilter::Cwd {
                        values: vec![Some("C:\\a".to_string())],
                    },
                    SessionResultFilter::CreatedAt {
                        from: Some(20.0),
                        to: Some(20.0),
                    },
                    SessionResultFilter::Parent {
                        values: vec![Some(session_id("parent"))],
                    },
                    SessionResultFilter::Availability {
                        values: vec![SessionAvailability::Live],
                    },
                ]),
                event_filters: Some(vec![SessionEventResultFilter::Surface {
                    values: vec![SessionEventSurface::Shadowed],
                }]),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("grouped");
    assert_eq!(grouped.items.len(), 1);
    assert_eq!(grouped.items[0].record.header.id, session_id("a"));
    assert!(grouped.items[0].record.live);
    assert!(!grouped.items[0].record.persisted);
    assert_eq!(grouped.items[0].best_match.seq, 0u64);
    assert_eq!(
        grouped.items[0].best_match.surface,
        SessionEventSurface::Shadowed
    );
}

#[tokio::test]
async fn uses_literal_phrase_tokens_stable_ties_and_bounded_snippets() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        default_limit: Some(10),
        max_limit: Some(10),
        snippet_chars: Some(5),
        ..Default::default()
    });
    for (id, text) in [
        ("a", "😀😀 alpha beta BRAID 😀😀"),
        ("b", "alpha beta"),
        ("c", "alpha middle beta"),
        ("d", "alpha beta"),
        ("operator", "needle OR absent"),
        ("only", "needle only"),
        ("quote", "say \"needle\" exactly"),
    ] {
        create_session(
            &store,
            &ctx,
            id,
            message_events(text, 10),
            1,
            CreateSessionMeta::default(),
        )
        .await;
    }

    let phrase = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "alpha beta".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("phrase");
    let ids: Vec<String> = phrase
        .items
        .iter()
        .map(|item| item.record.header.id.as_str().to_string())
        .collect();
    assert_eq!(ids, vec!["b", "d", "a"]);
    assert!(phrase
        .items
        .iter()
        .all(|item| item.best_match.snippet.chars().count() <= 5));

    let absent = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "AI".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("absent");
    assert!(absent.items.is_empty());

    let operator = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle OR absent".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("operator");
    assert_eq!(operator.items.len(), 1);
    assert_eq!(operator.items[0].record.header.id, session_id("operator"));

    let quote = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "say \"needle\"".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("quote");
    assert_eq!(quote.items.len(), 1);
    assert_eq!(quote.items[0].record.header.id, session_id("quote"));

    let star = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "*".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("star");
    assert!(star.items.is_empty());
}

#[tokio::test]
async fn positions_snippets_from_fts5_matches_across_diacritics_and_punctuation() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        snippet_chars: Some(14),
        ..Default::default()
    });
    create_session(
        &store,
        &ctx,
        "snippet",
        message_events("long long long—café,\nnext value", 10),
        1,
        CreateSessionMeta::default(),
    )
    .await;

    let page = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("snippet")),
                query: "CAFE".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("page");
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].snippet.contains("café"));
    assert!(page.items[0].snippet.contains('—'));
    assert!(!page.items[0].snippet.contains('\n'));
    assert!(page.items[0].snippet.chars().count() <= 14);
}

#[tokio::test]
async fn binds_cursors_to_requests_and_invalidates_within_scope() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        default_limit: Some(1),
        max_limit: Some(5),
        ..Default::default()
    });
    let mut target_events = message_events("needle one", 10);
    target_events.push(user_event("needle two", 1, 11, SurfaceOp::Append));
    target_events.push(user_event("needle three", 2, 12, SurfaceOp::Append));
    let target = create_session(
        &store,
        &ctx,
        "target",
        target_events,
        1,
        CreateSessionMeta::default(),
    )
    .await;
    create_session(
        &store,
        &ctx,
        "other",
        message_events("needle other", 10),
        1,
        CreateSessionMeta::default(),
    )
    .await;

    let event_page = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("target")),
                query: "needle".to_string(),
                limit: Some(1),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("event page");
    let session_page = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                limit: Some(1),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("session page");
    let event_cursor = event_page.next_cursor.clone().expect("event cursor");
    let session_cursor = session_page.next_cursor.clone().expect("session cursor");

    // A mangled offset is an invalid cursor.
    let mangled = unsafe_mangle_offset(&event_cursor, 1e100);
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("target")),
                query: "needle".to_string(),
                limit: Some(1),
                cursor: Some(mangled),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("invalid");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidCursor);

    // Walk the event cursor to the end.
    let mut keys: Vec<String> = event_page
        .items
        .iter()
        .map(|item| format!("{}:{}", item.session_id.as_str(), item.seq))
        .collect();
    let mut cursor = event_page.next_cursor.clone();
    while let Some(next) = cursor {
        let page = engine
            .search_events(
                &SessionEventSearchRequest {
                    session_id: Some(session_id("target")),
                    query: "needle".to_string(),
                    limit: Some(1),
                    cursor: Some(next),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("event walk");
        keys.extend(
            page.items
                .iter()
                .map(|item| format!("{}:{}", item.session_id.as_str(), item.seq)),
        );
        cursor = page.next_cursor;
    }
    assert_eq!(keys.len(), 3);
    let unique: std::collections::HashSet<&String> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len());

    // Walk the session cursor to the end.
    let mut ids: Vec<String> = session_page
        .items
        .iter()
        .map(|item| item.record.header.id.as_str().to_string())
        .collect();
    let mut cursor = session_page.next_cursor.clone();
    while let Some(next) = cursor {
        let page = engine
            .search_sessions(
                &SessionSearchRequest {
                    query: "needle".to_string(),
                    limit: Some(1),
                    cursor: Some(next),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("session walk");
        ids.extend(
            page.items
                .iter()
                .map(|item| item.record.header.id.as_str().to_string()),
        );
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), 2);
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());

    // An unrelated live session invalidates session pages but not the target
    // event page.
    create_session(
        &store,
        &ctx,
        "unrelated",
        message_events("needle unrelated", 20),
        1,
        CreateSessionMeta::default(),
    )
    .await;
    let ok = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("target")),
                query: "needle".to_string(),
                limit: Some(1),
                cursor: Some(event_cursor.clone()),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("event page still valid");
    assert_eq!(ok.items[0].session_id, session_id("target"));
    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                limit: Some(1),
                cursor: Some(session_cursor),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("stale");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryStaleCursor);

    // A different request fingerprint is an invalid cursor.
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("target")),
                query: "different".to_string(),
                limit: Some(1),
                cursor: Some(event_cursor.clone()),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("invalid");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidCursor);

    // Appending to the target invalidates its event page.
    target
        .append(
            "user/message",
            serde_json::to_value(dsh_llm::create_user_message(
                vec![dsh_llm::ContentBlock::Text {
                    text: "needle four".to_string(),
                }],
                dsh_llm::MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            ))
            .expect("message"),
            Some(SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("append");
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("target")),
                query: "needle".to_string(),
                limit: Some(1),
                cursor: Some(event_cursor),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("stale");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryStaleCursor);
}

/// Decode a cursor, replace its offset, and re-encode it (test-only
/// `replaceCursorOffset`).
fn unsafe_mangle_offset(
    cursor: &dsh_session_query::SessionSearchCursor,
    offset: f64,
) -> dsh_session_query::SessionSearchCursor {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.as_str())
        .expect("decode");
    let mut payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    payload["offset"] =
        serde_json::Value::Number(serde_json::Number::from_f64(offset).expect("number"));
    dsh_session_query::session_search_cursor(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).expect("reencode")),
    )
}

#[tokio::test]
async fn mounts_persistence_dynamically_shadows_and_reveals() {
    let shared = header(
        "shared",
        10,
        CreateSessionMeta {
            cwd: Some("C:\\work".to_string()),
            ..Default::default()
        },
    );
    let durable = header("durable", 5, CreateSessionMeta::default());
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(shared.clone(), message_events("persisted needle", 1));
    persistence.set(durable.clone(), message_events("durable needle", 1));

    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    let empty = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "durable".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("empty before mount");
    assert!(empty.items.is_empty());

    let disposer = mount_persistence(&ctx, &persistence);
    let found = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "durable".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("durable");
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].record.header.id, durable.id);
    assert!(!found.items[0].record.live);
    assert!(found.items[0].record.persisted);

    // A live owner of `shared` shadows the persisted log.
    let live = store
        .prepare(
            Some(shared.id.clone()),
            Some(CreateSessionOptions {
                seed: Some(message_events("live needle", 1)),
                meta: Some(CreateSessionMeta {
                    created_at: Some(10),
                    cwd: Some("C:\\work".to_string()),
                    ..Default::default()
                }),
            }),
        )
        .expect("prepare");
    let detach = store.enter(&live).expect("enter");
    store.announce(&live).await.expect("announce");

    let hidden = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("hidden");
    assert!(hidden.items.is_empty());
    let live_hit = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "live".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("live");
    assert_eq!(live_hit.items.len(), 1);
    assert!(live_hit.items[0].record.live);
    assert!(live_hit.items[0].record.persisted);
    assert_eq!(live_hit.items[0].record.header.id, shared.id);

    detach().await;
    let revealed = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("revealed");
    assert_eq!(revealed.items.len(), 1);
    assert!(!revealed.items[0].record.live);
    assert!(revealed.items[0].record.persisted);

    unmount(disposer).await;
    // Let the optional-persistence child fiber reset the binding cell.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let gone = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "durable".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("gone");
    assert!(gone.items.is_empty());
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(durable.id.clone()),
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("not found");
    assert_eq!(
        code(&error),
        SessionQueryErrorCode::SessionQuerySessionNotFound
    );
}

#[tokio::test]
async fn does_not_load_a_persisted_log_while_the_same_session_is_live() {
    let shared = header("checkpointed-live", 10, CreateSessionMeta::default());
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(shared.clone(), message_events("persisted needle", 1));

    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    let live = store
        .prepare(
            Some(shared.id.clone()),
            Some(CreateSessionOptions {
                seed: Some(message_events("live needle", 1)),
                meta: Some(CreateSessionMeta {
                    created_at: Some(10),
                    ..Default::default()
                }),
            }),
        )
        .expect("prepare");
    let detach = store.enter(&live).expect("enter");
    store.announce(&live).await.expect("announce");
    let _disposer = mount_persistence(&ctx, &persistence);

    let hits = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "live".to_string(),
                session_filters: Some(vec![SessionResultFilter::Availability {
                    values: vec![SessionAvailability::Persisted],
                }]),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("hits");
    assert_eq!(hits.items.len(), 1);
    assert!(hits.items[0].record.live);
    assert!(hits.items[0].record.persisted);
    assert_eq!(persistence.inspections("checkpointed-live"), 0);

    detach().await;
    let revealed = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("revealed");
    assert_eq!(revealed.items.len(), 1);
    assert!(!revealed.items[0].record.live);
    assert_eq!(persistence.inspections("checkpointed-live"), 1);
}

#[tokio::test]
async fn retries_when_the_snapshot_population_changes_during_observation() {
    let first = header("first", 1, CreateSessionMeta::default());
    let added = header("added-during-list", 1, CreateSessionMeta::default());
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(first.clone(), message_events("first needle", 1));

    let (ctx, _search, engine, _store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    let _disposer = mount_persistence(&ctx, &persistence);
    let shared_persistence = persistence.clone();
    let added_meta = added.clone();
    persistence.set_snapshot_effect(Some(Arc::new(move || {
        shared_persistence.set_snapshot_effect(None);
        shared_persistence.set(added_meta.clone(), message_events("added needle", 1));
    })));

    let page = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("page");
    let mut ids: Vec<String> = page
        .items
        .iter()
        .map(|item| item.record.header.id.as_str().to_string())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["added-during-list", "first"]);
    assert_eq!(persistence.inspections("first"), 2);
    assert_eq!(persistence.inspections("added-during-list"), 1);
}

#[tokio::test]
async fn fails_after_one_retry_when_persistence_snapshots_keep_changing() {
    let durable = header("continuous-mutation", 1, CreateSessionMeta::default());
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(durable.clone(), message_events("durable needle", 1));

    let (ctx, _search, engine, _store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    let _disposer = mount_persistence(&ctx, &persistence);
    let shared_persistence = persistence.clone();
    persistence.set_snapshot_effect(Some(Arc::new(move || {
        shared_persistence.set(durable.clone(), message_events("durable needle again", 1));
    })));

    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("unstable");
    assert_eq!(
        code(&error),
        SessionQueryErrorCode::SessionQueryPersistenceFailed
    );
}

#[tokio::test]
async fn rejects_immutable_header_conflicts_between_live_and_persisted_sources() {
    let shared = header(
        "conflict",
        10,
        CreateSessionMeta {
            delegation_depth: Some(1),
            ..Default::default()
        },
    );
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(shared.clone(), message_events("persisted needle", 1));

    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    let _disposer = mount_persistence(&ctx, &persistence);
    create_session(
        &store,
        &ctx,
        "conflict",
        message_events("live needle", 1),
        10,
        CreateSessionMeta {
            delegation_depth: Some(2),
            ..Default::default()
        },
    )
    .await;

    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("conflict");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQuerySourceConflict);
}

#[tokio::test]
async fn invalidates_session_cursors_after_transient_persistence_topology_changes() {
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();

    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        default_limit: Some(1),
        max_limit: Some(5),
        ..Default::default()
    });
    create_session(
        &store,
        &ctx,
        "first",
        message_events("needle first", 1),
        1,
        CreateSessionMeta::default(),
    )
    .await;
    create_session(
        &store,
        &ctx,
        "second",
        message_events("needle second", 1),
        1,
        CreateSessionMeta::default(),
    )
    .await;
    let page = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                limit: Some(1),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("page");
    let cursor = page.next_cursor.expect("cursor");

    let disposer = mount_persistence(&ctx, &persistence);
    unmount(disposer).await;
    // Let the optional-persistence child fiber reset the binding cell.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                limit: Some(1),
                cursor: Some(cursor),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("stale");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryStaleCursor);
}

#[tokio::test]
async fn searches_at_the_fts5_outer_predicate_boundary_and_rejects_above_it() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    create_session(
        &store,
        &ctx,
        "predicate-boundary",
        message_events("needle", 1),
        1,
        CreateSessionMeta {
            cwd: Some("C:\\work".to_string()),
            ..Default::default()
        },
    )
    .await;
    let session_filters: Vec<SessionResultFilter> = (0..14)
        .map(|_| SessionResultFilter::Cwd {
            values: vec![Some("C:\\work".to_string()), None],
        })
        .collect();
    let event_filters: Vec<SessionEventResultFilter> = (0..13)
        .map(|_| SessionEventResultFilter::Type {
            values: vec!["user/message".to_string()],
        })
        .collect();

    let ok = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                session_filters: Some(session_filters),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("boundary sessions");
    assert_eq!(ok.items.len(), 1);
    let ok = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("predicate-boundary")),
                query: "needle".to_string(),
                filters: Some(event_filters),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("boundary events");
    assert_eq!(ok.items.len(), 1);

    // 14 event predicates + 1 fixed target predicate exceeds the budget.
    let over: Vec<SessionEventResultFilter> = (0..14)
        .map(|_| SessionEventResultFilter::Type {
            values: vec!["user/message".to_string()],
        })
        .collect();
    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("predicate-boundary")),
                query: "needle".to_string(),
                filters: Some(over),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);
}

#[tokio::test]
async fn rejects_aggregate_filter_bindings_above_sqlites_portable_variable_limit() {
    let (ctx, _search, engine, store) = live_context(Config {
        path: ":memory:".to_string(),
        ..Default::default()
    });
    create_session(
        &store,
        &ctx,
        "binding-limit",
        message_events("needle", 1),
        1,
        CreateSessionMeta::default(),
    )
    .await;
    let ids: Vec<dsh_session::SessionId> = (0..16_383)
        .map(|index| session_id(format!("binding-{index}")))
        .collect();
    let types: Vec<String> = (0..16_383).map(|_| "user/message".to_string()).collect();
    let surfaces: Vec<SessionEventSurface> =
        (0..16_383).map(|_| SessionEventSurface::Current).collect();

    let error = engine
        .search_sessions(
            &SessionSearchRequest {
                query: "needle".to_string(),
                session_filters: Some(vec![SessionResultFilter::Id {
                    values: ids.clone(),
                }]),
                event_filters: Some(vec![SessionEventResultFilter::Type {
                    values: types.clone(),
                }]),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);

    let error = engine
        .search_events(
            &SessionEventSearchRequest {
                session_id: Some(session_id("binding-limit")),
                query: "needle".to_string(),
                filters: Some(vec![
                    SessionEventResultFilter::Type { values: types },
                    SessionEventResultFilter::Surface { values: surfaces },
                ]),
                ..Default::default()
            },
            None,
        )
        .await
        .err()
        .expect("rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);
}

#[tokio::test]
async fn drops_connection_local_live_overlays_on_reopen_and_retains_persistent_bases() {
    let path = std::env::temp_dir().join(format!("dsh-search-{}.db", uuid::Uuid::new_v4()));
    let shared = header("shared", 10, CreateSessionMeta::default());
    let persistence = TestPersistence::new(&Context::root());
    persistence.reset();
    persistence.set(shared.clone(), message_events("persisted needle", 1));

    {
        let (ctx, search, engine, store) = live_context(Config {
            path: path.to_string_lossy().to_string(),
            ..Default::default()
        });
        create_session(
            &store,
            &ctx,
            "shared",
            message_events("live needle", 1),
            10,
            CreateSessionMeta::default(),
        )
        .await;
        let _disposer = mount_persistence(&ctx, &persistence);
        let live = engine
            .search_events(
                &SessionEventSearchRequest {
                    session_id: Some(session_id("shared")),
                    query: "live".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("live");
        assert_eq!(live.items.len(), 1);
        search.close().await;
    }

    {
        let (ctx, search, engine, _store) = live_context(Config {
            path: path.to_string_lossy().to_string(),
            ..Default::default()
        });
        let _disposer = mount_persistence(&ctx, &persistence);
        let live = engine
            .search_sessions(
                &SessionSearchRequest {
                    query: "live".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("no live");
        assert!(live.items.is_empty());
        let persisted = engine
            .search_sessions(
                &SessionSearchRequest {
                    query: "persisted".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("persisted");
        assert_eq!(persisted.items.len(), 1);
        assert_eq!(persisted.items[0].record.header.id, shared.id);
        assert!(!persisted.items[0].record.live);
        assert!(persisted.items[0].record.persisted);
        search.close().await;
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}

#[tokio::test]
async fn validates_configuration_and_rejects_invalid_values() {
    for config in [
        Config {
            path: "".to_string(),
            ..Default::default()
        },
        Config {
            path: ":memory:".to_string(),
            default_limit: Some(0),
            ..Default::default()
        },
        Config {
            path: ":memory:".to_string(),
            max_limit: Some(0),
            ..Default::default()
        },
        Config {
            path: ":memory:".to_string(),
            snippet_chars: Some(0),
            ..Default::default()
        },
        Config {
            path: ":memory:".to_string(),
            persisted_inspect_concurrency: Some(0),
            ..Default::default()
        },
        Config {
            path: ":memory:".to_string(),
            default_limit: Some(3),
            max_limit: Some(2),
            ..Default::default()
        },
    ] {
        let error = dsh_session_query_sqlite::resolve_config(&config)
            .err()
            .expect("invalid config");
        assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidConfig);
    }
    let resolved = dsh_session_query_sqlite::resolve_config(&Config {
        path: ":memory:".to_string(),
        ..Default::default()
    })
    .expect("valid");
    assert_eq!(resolved.open_at, OpenAt::Startup);
    assert_eq!(resolved.default_limit, 20);
    assert_eq!(resolved.max_limit, 100);
    assert_eq!(resolved.snippet_chars, 240);
    assert_eq!(resolved.persisted_inspect_concurrency, 4);
}
