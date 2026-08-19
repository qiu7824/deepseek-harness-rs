//! SessionProjectionCache behavior. Rust port of the core
//! `packages/session/session-projection-cache/tests/cache.spec.ts`
//! behaviors: mandatory-point writes, throttling, fail-soft durability, and
//! the cold-read ladder.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cordis::{ArcValue, Context, Plugin, PluginError, arc};
use parking_lot::Mutex;
use serde_json::{Value as JsonValue, json};

use dsh_session::{Session, SessionEvent, SessionHeader, SessionStore, session_id};
use dsh_session_persistence::{SessionInspection, SessionLocation, SessionPersistenceApi};
use dsh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use dsh_session_projection_cache::{Config, SessionProjectionCache};
use dsh_storage::Storage;
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_test_support::{MemoryMediaPool, MemoryStorageBackend};

fn marks_unit(state_version: u64) -> ProjectionDefinition {
    let init: Arc<dyn Fn() -> ArcValue + Send + Sync> = Arc::new(|| arc(JsonValue::Null));
    let apply: Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync> =
        Arc::new(|state, event| {
            if event.type_ == "cache-test/mark" {
                arc(event.data.clone())
            } else {
                state.clone()
            }
        });
    let view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync> = Arc::new(|state| {
        let state: &JsonValue = cordis::downcast(state).expect("marks state");
        match state {
            JsonValue::Null => arc(json!({"marks": []})),
            other => arc(other.clone()),
        }
    });
    let schema: Arc<dyn Fn(&ArcValue) -> Result<JsonValue, String> + Send + Sync> =
        Arc::new(|value| {
            let json: &JsonValue = cordis::downcast(value).expect("marks view");
            let marks = json
                .get("marks")
                .and_then(|marks| marks.as_array())
                .ok_or_else(|| "marks view must carry a marks array".to_string())?;
            if marks.iter().all(|mark| mark.is_string()) {
                Ok(json.clone())
            } else {
                Err("marks entries must be strings".to_string())
            }
        });
    ProjectionDefinition {
        key: "cache-test/marks".to_string(),
        schema,
        init,
        apply,
        view,
        state_version,
    }
}

/// A persistence double serving readFrom over a fixed per-id stored log
/// (headers stamp createdAt 0).
struct FakePersistence {
    logs: Mutex<HashMap<String, Vec<SessionEvent>>>,
    read_from_calls: Mutex<Vec<(String, u64)>>,
}

impl FakePersistence {
    fn new(logs: HashMap<String, Vec<SessionEvent>>) -> Self {
        Self {
            logs: Mutex::new(logs),
            read_from_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SessionPersistenceApi for FakePersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, _meta: SessionHeader) -> Result<(), String> {
        unimplemented!("fake read-only persistence")
    }

    async fn append(
        &self,
        _id: &dsh_session::SessionId,
        _events: &[SessionEvent],
    ) -> Result<(), String> {
        unimplemented!("fake read-only persistence")
    }

    async fn load(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        let events = self
            .logs
            .lock()
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| format!("session \"{id}\" not found"))?;
        Ok(SessionInspection {
            meta: SessionHeader {
                version: 0,
                id: id.clone(),
                created_at: 0,
                cwd: None,
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: None,
            },
            events,
        })
    }

    async fn inspect(&self, id: &dsh_session::SessionId) -> Result<SessionInspection, String> {
        self.load(id).await
    }

    async fn read_from(
        &self,
        id: &dsh_session::SessionId,
        from_seq: u64,
    ) -> Result<dsh_session_persistence::SessionReadFromResult, String> {
        self.read_from_calls
            .lock()
            .push((id.as_str().to_string(), from_seq));
        let events = self
            .logs
            .lock()
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| format!("session \"{id}\" not found"))?;
        Ok(dsh_session_persistence::SessionReadFromResult {
            meta: SessionHeader {
                version: 0,
                id: id.clone(),
                created_at: 0,
                cwd: None,
                parent_session: None,
                seed_length: None,
                origin: None,
                delegation_depth: None,
                agent_preset: None,
            },
            events: events
                .into_iter()
                .filter(|event| event.seq >= from_seq)
                .collect(),
        })
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        unimplemented!("fake read-only persistence")
    }

    async fn list_snapshots(
        &self,
    ) -> Result<Vec<dsh_session_persistence::SessionPersistenceSnapshot>, String> {
        unimplemented!("fake read-only persistence")
    }

    fn ctx(&self) -> &Context {
        unimplemented!("fake read-only persistence")
    }
}

impl cordis::Service for FakePersistence {
    fn service_name(&self) -> &'static str {
        "sessionPersistence"
    }
}

struct Harness {
    ctx: Context,
    pool: Arc<MemoryMediaPool>,
    logs: Arc<FakePersistence>,
    store: Arc<SessionStore>,
    cache: Arc<SessionProjectionCache>,
}

fn header_of(id: &str, created_at: u64, cwd: Option<&str>) -> SessionHeader {
    SessionHeader {
        version: 0,
        id: session_id(id),
        created_at,
        cwd: cwd.map(|cwd| cwd.to_string()),
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    }
}

async fn harness(
    pool: Arc<MemoryMediaPool>,
    logs: HashMap<String, Vec<SessionEvent>>,
    config: Config,
    state_version: u64,
) -> Harness {
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = MemoryStorageBackend::with_shared_pool(pool.clone());
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
    let store = SessionStore::install(&ctx);
    let registry = SessionProjectionRegistry::install(&ctx);
    let _ = registry
        .register(&ctx, marks_unit(state_version))
        .expect("register marks unit");
    let persistence = Arc::new(FakePersistence::new(logs));
    let cache = SessionProjectionCache::install(&ctx, config, &facility, persistence.clone())
        .expect("install cache");
    Harness {
        ctx,
        pool,
        logs: persistence,
        store,
        cache,
    }
}

fn default_config() -> Config {
    Config {
        write_every_events: 100,
        write_interval_ms: 60_000,
    }
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

fn mark(session: &Session, marks: &[&str]) -> SessionEvent {
    session
        .append("cache-test/mark", json!({ "marks": marks }), None)
        .expect("append")
}

fn end_turn(session: &Session) -> SessionEvent {
    session
        .append(
            "turn/end",
            dsh_session::turn_end_data(1, &dsh_session::TurnEndReason::Completed),
            None,
        )
        .expect("append")
}

fn stored_record(pool: &MemoryMediaPool, id: &str) -> Option<JsonValue> {
    pool.media
        .lock()
        .get("session_projcache")
        .and_then(|medium| medium.tables.get("sessions"))
        .and_then(|records| records.get(id))
        .cloned()
}

fn stored_rows(pool: &MemoryMediaPool, id: &str) -> Option<JsonValue> {
    stored_record(pool, id).and_then(|record| record.get("rows").cloned())
}

async fn live_session(store: &SessionStore, ctx: &Context, id: &str) -> Session {
    store
        .create(
            &ctx,
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create")
}

#[tokio::test(flavor = "current_thread")]
async fn writes_a_durable_checkpoint_at_turn_end() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    let session = live_session(&h.store, &h.ctx, "turn-end").await;
    mark(&session, &["a"]);
    assert!(stored_rows(&pool, "turn-end").is_none());
    let end = end_turn(&session);
    settle().await;
    let rows = stored_rows(&pool, "turn-end").expect("stored");
    assert_eq!(
        rows["cache-test/marks"],
        json!({"ver": 1, "seq": end.seq, "val": {"marks": ["a"]}})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn writes_at_session_disposal() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    // Sessions dispose with their owning fiber: create in a child plugin.
    struct Owner {
        session: Mutex<Option<Session>>,
    }
    #[async_trait]
    impl Plugin for Owner {
        fn name(&self) -> Option<&'static str> {
            Some("owner")
        }

        fn inject(&self) -> cordis::InjectSpec {
            cordis::InjectSpec::new(["sessions"])
        }

        async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
            let store = ctx
                .get_typed::<Arc<SessionStore>>("sessions", false)
                .expect("sessions");
            let session = store
                .create(
                    ctx,
                    Some(session_id("detach")),
                    Some(dsh_session::CreateSessionOptions::default()),
                )
                .await
                .expect("create");
            *self.session.lock() = Some(session);
            Ok(())
        }
    }
    let owner = Arc::new(Owner {
        session: Mutex::new(None),
    });
    let fiber = h.ctx.plugin(owner.clone(), arc(()));
    fiber.settle().await.expect("settle");
    let session = owner.session.lock().take().expect("session");
    mark(&session, &["live"]);
    fiber.dispose().await;
    settle().await;
    let rows = stored_rows(&pool, "detach").expect("stored");
    assert_eq!(rows["cache-test/marks"]["val"], json!({"marks": ["live"]}));
}

#[tokio::test(flavor = "current_thread")]
async fn flushes_when_the_in_turn_event_count_reaches_the_configured_threshold() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(
        pool.clone(),
        HashMap::new(),
        Config {
            write_every_events: 3,
            write_interval_ms: 60_000,
        },
        1,
    )
    .await;
    let session = live_session(&h.store, &h.ctx, "count").await;
    mark(&session, &["1"]);
    mark(&session, &["2"]);
    settle().await;
    assert!(stored_rows(&pool, "count").is_none());
    mark(&session, &["3"]);
    settle().await;
    let rows = stored_rows(&pool, "count").expect("stored");
    assert_eq!(rows["cache-test/marks"]["val"], json!({"marks": ["3"]}));
}

#[tokio::test(flavor = "current_thread")]
async fn coalesces_count_threshold_flushes_before_spawned_tasks_are_polled() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(
        pool,
        HashMap::new(),
        Config {
            write_every_events: 100,
            write_interval_ms: 60_000,
        },
        1,
    )
    .await;
    let flushes = Arc::new(AtomicUsize::new(0));
    let flushes_for_listener = Arc::clone(&flushes);
    h.ctx
        .on(
            "session/flush",
            Arc::new(move |_ctx, _args| {
                let flushes = Arc::clone(&flushes_for_listener);
                Box::pin(async move {
                    flushes.fetch_add(1, Ordering::SeqCst);
                    None
                })
            }),
            Default::default(),
        )
        .await;
    let session = live_session(&h.store, &h.ctx, "count-coalescing").await;

    for event in 1..=500 {
        mark(&session, &[&event.to_string()]);
    }
    settle().await;

    assert_eq!(
        flushes.load(Ordering::SeqCst),
        5,
        "one count-triggered flush is allowed per 100 newly dirty events"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn flushes_on_the_configured_interval_when_the_count_threshold_is_not_reached() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(
        pool.clone(),
        HashMap::new(),
        Config {
            write_every_events: 100,
            write_interval_ms: 250,
        },
        1,
    )
    .await;
    let session = live_session(&h.store, &h.ctx, "interval").await;
    mark(&session, &["slow"]);
    // The timer task spawns from the event listener: one settle lets it
    // run and create its sleep at the paused clock's T=0.
    settle().await;
    tokio::time::advance(Duration::from_millis(249)).await;
    settle().await;
    assert!(stored_rows(&pool, "interval").is_none());
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    let rows = stored_rows(&pool, "interval").expect("stored");
    assert_eq!(rows["cache-test/marks"]["val"], json!({"marks": ["slow"]}));
}

#[tokio::test(flavor = "current_thread")]
async fn write_on_a_never_dirty_session_checkpoints_directly_and_rejects_a_non_json_unit_state() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    let clean = live_session(&h.store, &h.ctx, "clean-write").await;
    h.cache.write(&clean).await.expect("write");
    let rows = stored_rows(&pool, "clean-write").expect("stored");
    assert_eq!(
        rows["cache-test/marks"],
        json!({"ver": 1, "seq": -1, "val": null})
    );

    // A unit whose state violates the plain-JSON contract fails the write
    // loud (the registry's checkpoint downcast panics 鈥?the TS rejection's
    // Rust form).
    let registry = h
        .ctx
        .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
        .expect("registry")
        .as_ref()
        .clone();
    let _ = registry
        .register(
            &h.ctx,
            ProjectionDefinition {
                key: "cache-test/marks2".to_string(),
                schema: Arc::new(|_value| Ok(json!(null))),
                init: Arc::new(|| arc(42_i64)),
                apply: Arc::new(|state, _event| state.clone()),
                view: Arc::new(|_state| arc(json!(null))),
                state_version: 1,
            },
        )
        .expect("register bad unit");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        futures::executor::block_on(h.cache.write(&clean))
    }));
    assert!(outcome.is_err(), "non-JSON unit state must fail the write");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn plugin_disposal_clears_armed_interval_timers() {
    let pool = Arc::new(MemoryMediaPool::new());
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = MemoryStorageBackend::with_shared_pool(pool.clone());
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
    let store = SessionStore::install(&ctx);
    let registry = SessionProjectionRegistry::install(&ctx);
    let _ = registry.register(&ctx, marks_unit(1)).expect("register");
    let persistence = Arc::new(FakePersistence::new(HashMap::new()));
    ctx.register_service(persistence.clone());
    let plugin = dsh_session_projection_cache::SessionProjectionCachePlugin {
        config: Config {
            write_every_events: 100,
            write_interval_ms: 5000,
        },
        facility,
        persistence,
    };
    let fiber = ctx.plugin(Arc::new(plugin), arc(()));
    fiber.settle().await.expect("settle");
    let armed = live_session(&store, &ctx, "armed").await;
    let cleaned = live_session(&store, &ctx, "cleaned").await;
    mark(&armed, &["pending"]); // timer armed, no write yet
    mark(&cleaned, &["done"]);
    end_turn(&cleaned); // mandatory write; markClean leaves the entry
    tokio::time::advance(Duration::from_millis(0)).await;
    settle().await;
    fiber.dispose().await;
    // The armed timer died with the plugin: advancing time writes nothing.
    tokio::time::advance(Duration::from_millis(10_000)).await;
    settle().await;
    assert!(stored_rows(&pool, "armed").is_none());
    let rows = stored_rows(&pool, "cleaned").expect("mandatory write");
    assert_eq!(rows["cache-test/marks"]["val"], json!({"marks": ["done"]}));
}

#[tokio::test(flavor = "current_thread")]
async fn contains_a_durable_write_failure_and_the_next_write_self_heals() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    let session = live_session(&h.store, &h.ctx, "fail-soft").await;
    mark(&session, &["x"]);
    pool.fail_next_writes
        .store(1, std::sync::atomic::Ordering::SeqCst);
    end_turn(&session);
    settle().await;
    assert!(stored_rows(&pool, "fail-soft").is_none());
    // Self-heal: the next mandatory point writes the current cut.
    mark(&session, &["y"]);
    end_turn(&session);
    settle().await;
    let rows = stored_rows(&pool, "fail-soft").expect("stored");
    assert_eq!(rows["cache-test/marks"]["val"], json!({"marks": ["y"]}));
}

fn stored_log(marks: &[Vec<&str>]) -> Vec<SessionEvent> {
    let mut events = vec![SessionEvent {
        type_: "turn/start".to_string(),
        seq: 0,
        time: 0,
        data: json!({"turn": 1}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }];
    for (index, marks) in marks.iter().enumerate() {
        events.push(SessionEvent {
            type_: "cache-test/mark".to_string(),
            seq: events.len() as u64,
            time: events.len() as i64,
            data: json!({"marks": marks}),
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        });
        let _ = index;
    }
    events.push(SessionEvent {
        type_: "turn/end".to_string(),
        seq: events.len() as u64,
        time: events.len() as i64,
        data: json!({"turn": 1, "reason": {"kind": "completed"}}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    });
    events
}

fn seed_row(pool: &MemoryMediaPool, id: &str, row: JsonValue, identity: JsonValue) {
    pool.versions
        .lock()
        .insert("session_projcache".to_string(), 3);
    let mut tables = HashMap::new();
    let mut sessions = HashMap::new();
    sessions.insert(
        id.to_string(),
        json!({"identity": identity, "rows": {"cache-test/marks": row}}),
    );
    tables.insert("sessions".to_string(), sessions);
    pool.media.lock().insert(
        "session_projcache".to_string(),
        dsh_storage_test_support::MemoryMedium {
            tables,
            global: JsonValue::Null,
        },
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serves_a_cold_session_from_the_cache_row_plus_a_bounded_tail_read_and_writes_back() {
    let pool = Arc::new(MemoryMediaPool::new());
    let mut logs = HashMap::new();
    logs.insert("cold".to_string(), stored_log(&[vec!["a"], vec!["a", "b"]]));
    seed_row(
        &pool,
        "cold",
        json!({"ver": 1, "seq": 1, "val": {"marks": ["a"]}}),
        json!({"createdAt": 0}),
    );
    let h = harness(pool.clone(), logs, default_config(), 1).await;
    let id = session_id("cold");
    let snapshot = h.cache.cold_snapshot(&id).await.expect("cold snapshot");
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["a", "b"]}))
    );
    assert_eq!(snapshot.as_of_seq, 3);
    let calls = h.logs.read_from_calls.lock();
    assert_eq!(*calls, vec![("cold".to_string(), 1)]);
    drop(calls);
    let rows = stored_rows(&pool, "cold").expect("write-back");
    assert_eq!(
        rows["cache-test/marks"],
        json!({"ver": 1, "seq": 3, "val": {"marks": ["a", "b"]}})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn discards_a_version_mismatched_row_and_refolds_the_full_log() {
    let pool = Arc::new(MemoryMediaPool::new());
    let mut logs = HashMap::new();
    logs.insert("bumped".to_string(), stored_log(&[vec!["a"]]));
    seed_row(
        &pool,
        "bumped",
        json!({"ver": 1, "seq": 2, "val": {"marks": ["stale"]}}),
        json!({"createdAt": 0}),
    );
    let h = harness(pool.clone(), logs, default_config(), 2).await;
    let snapshot = h
        .cache
        .cold_snapshot(&session_id("bumped"))
        .await
        .expect("cold snapshot");
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["a"]}))
    );
    let calls = h.logs.read_from_calls.lock();
    assert_eq!(*calls, vec![("bumped".to_string(), 0)]);
}

#[tokio::test(flavor = "current_thread")]
async fn detects_a_log_shrunk_below_the_row_watermark_and_degrades_to_one_full_re_read() {
    let pool = Arc::new(MemoryMediaPool::new());
    let mut logs = HashMap::new();
    logs.insert("shrunk".to_string(), stored_log(&[vec!["a"]]));
    seed_row(
        &pool,
        "shrunk",
        json!({"ver": 1, "seq": 9, "val": {"marks": ["ghost"]}}),
        json!({"createdAt": 0}),
    );
    let h = harness(pool.clone(), logs, default_config(), 1).await;
    let snapshot = h
        .cache
        .cold_snapshot(&session_id("shrunk"))
        .await
        .expect("cold snapshot");
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["a"]}))
    );
    assert_eq!(snapshot.as_of_seq, 2);
    let calls = h.logs.read_from_calls.lock();
    assert_eq!(
        *calls,
        vec![("shrunk".to_string(), 9), ("shrunk".to_string(), 0)]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn write_back_failure_is_contained_and_the_snapshot_is_still_served() {
    let pool = Arc::new(MemoryMediaPool::new());
    let mut logs = HashMap::new();
    logs.insert("soft".to_string(), stored_log(&[vec!["a"]]));
    let h = harness(pool.clone(), logs, default_config(), 1).await;
    pool.fail_next_writes
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let snapshot = h
        .cache
        .cold_snapshot(&session_id("soft"))
        .await
        .expect("cold snapshot");
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["a"]}))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_for_a_session_with_no_persisted_log() {
    let pool = Arc::new(MemoryMediaPool::new());
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    let error = h
        .cache
        .cold_snapshot(&session_id("absent"))
        .await
        .err()
        .expect("reject");
    assert!(error.contains("not found"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn discards_a_record_bound_to_a_different_log_lifecycle_and_refolds() {
    let pool = Arc::new(MemoryMediaPool::new());
    let mut logs = HashMap::new();
    logs.insert("reborn".to_string(), stored_log(&[vec!["real"]]));
    seed_row(
        &pool,
        "reborn",
        json!({"ver": 1, "seq": 2, "val": {"marks": ["phantom"]}}),
        json!({"createdAt": 999}),
    );
    let h = harness(pool.clone(), logs, default_config(), 1).await;
    let snapshot = h
        .cache
        .cold_snapshot(&session_id("reborn"))
        .await
        .expect("cold snapshot");
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["real"]}))
    );
    let stored = stored_record(&pool, "reborn").expect("write-back");
    assert_eq!(stored["identity"], json!({"createdAt": 0}));
}

#[tokio::test(flavor = "current_thread")]
async fn cached_snapshot_returns_undefined_when_every_stored_row_is_version_mismatched() {
    let pool = Arc::new(MemoryMediaPool::new());
    seed_row(
        &pool,
        "all-stale",
        json!({"ver": 99, "seq": 4, "val": {"marks": ["old"]}}),
        json!({"createdAt": 0}),
    );
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    assert!(
        h.cache
            .cached_snapshot(&header_of("all-stale", 0, None))
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn binds_identity_on_cwd_too() {
    let pool = Arc::new(MemoryMediaPool::new());
    seed_row(
        &pool,
        "homed",
        json!({"ver": 1, "seq": 2, "val": {"marks": ["w"]}}),
        json!({"createdAt": 0, "cwd": "/work"}),
    );
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    assert_eq!(
        h.cache
            .cached_snapshot(&header_of("homed", 0, Some("/work")))
            .expect("matching cwd")
            .values
            .get("cache-test/marks"),
        Some(&json!({"marks": ["w"]}))
    );
    assert!(
        h.cache
            .cached_snapshot(&header_of("homed", 0, Some("/elsewhere")))
            .is_none()
    );
    assert!(
        h.cache
            .cached_snapshot(&header_of("homed", 0, None))
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dates_an_empty_stored_log_at_minus_one_in_the_zero_units_topology() {
    let pool = Arc::new(MemoryMediaPool::new());
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = MemoryStorageBackend::with_shared_pool(pool.clone());
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
    let _store = SessionStore::install(&ctx);
    let _registry = SessionProjectionRegistry::install(&ctx);
    let mut logs = HashMap::new();
    logs.insert("empty".to_string(), Vec::new());
    let persistence = Arc::new(FakePersistence::new(logs));
    let cache = SessionProjectionCache::install(&ctx, default_config(), &facility, persistence)
        .expect("install");
    let snapshot = cache
        .cold_snapshot(&session_id("empty"))
        .await
        .expect("cold snapshot");
    assert_eq!(snapshot.as_of_seq, -1);
    assert!(snapshot.values.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cached_snapshot_serves_identity_matching_rows_and_refuses_unrelated_ones() {
    let pool = Arc::new(MemoryMediaPool::new());
    seed_row(
        &pool,
        "listed",
        json!({"ver": 1, "seq": 4, "val": {"marks": ["t"]}}),
        json!({"createdAt": 0}),
    );
    let h = harness(pool.clone(), HashMap::new(), default_config(), 1).await;
    let snapshot = h
        .cache
        .cached_snapshot(&header_of("listed", 0, None))
        .expect("matching");
    assert_eq!(snapshot.as_of_seq, 4);
    assert_eq!(
        snapshot.values.get("cache-test/marks"),
        Some(&json!({"marks": ["t"]}))
    );
    assert!(
        h.cache
            .cached_snapshot(&header_of("listed", 777, None))
            .is_none()
    );
    assert!(
        h.cache
            .cached_snapshot(&header_of("never-cached", 0, None))
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn holds_the_not_found_contract_with_zero_registered_units() {
    let pool = Arc::new(MemoryMediaPool::new());
    let ctx = Context::root();
    let hub = Storage::install(&ctx);
    let backend = MemoryStorageBackend::with_shared_pool(pool.clone());
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
    let _store = SessionStore::install(&ctx);
    let _registry = SessionProjectionRegistry::install(&ctx);
    let mut logs = HashMap::new();
    logs.insert("bare".to_string(), stored_log(&[vec!["a"]]));
    let persistence = Arc::new(FakePersistence::new(logs));
    let cache = SessionProjectionCache::install(&ctx, default_config(), &facility, persistence)
        .expect("install");
    let error = cache
        .cold_snapshot(&session_id("absent"))
        .await
        .err()
        .expect("not found");
    assert!(error.contains("not found"), "{error}");
    let snapshot = cache
        .cold_snapshot(&session_id("bare"))
        .await
        .expect("cold snapshot");
    assert_eq!(snapshot.as_of_seq, 2);
    assert!(snapshot.values.is_empty());
}
