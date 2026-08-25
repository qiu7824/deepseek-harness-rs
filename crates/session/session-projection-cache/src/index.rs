//! Persisted projection cache (`ctx.sessionProjectionCache`). Rust port of
//! `packages/session/session-projection-cache/src/index.ts`.
//!
//! The cache is a fold shortcut, never an authority: a row is possibly
//! stale (its `seq` says how stale) but never wrong, so every write path is
//! fail-soft (a lost write costs a longer tail replay on the next cold
//! read) and a `ver` mismatch discards the row instead of migrating it.
//!
//! # Deviations
//!
//! - The TS storage hub injects `storageDomain`/`sessionPersistence`; the
//!   Rust install takes the [`DomainFacility`] and an erased
//!   [`SessionPersistenceApi`] explicitly (the hub seam is not ported yet).
//! - The domain opens synchronously at install (blocked once; the TS
//!   `Service.init` generator is async) — install is the one-time I/O
//!   boundary.
//! - `put`'s non-JSON state rejection collapses: checkpoint state is
//!   already constrained to plain JSON by the registry (the loud failure
//!   moved to checkpoint time).
//! - The TS `setTimeout` interval trigger becomes a tokio timer task with
//!   an abort handle stored per dirty session.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value as JsonValue;

use cordis::{ArcValue, Context, EventOptions, Listener, Service, arc, downcast, make_disposer};
use dsh_session::{Session, SessionEvent, SessionHeader, SessionId, SessionStore};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_projection::{ProjectionCheckpoint, ProjectionSnapshot, SessionProjectionRegistry};
use dsh_storage_domain::{Domain, DomainFacility, KvTable};

use crate::spec::{CheckpointIdentity, CheckpointRecord, projection_cache_domain_spec, rows_of};

/// Plugin config (TS `Config`). Both throttle triggers are deployment
/// choices; the two mandatory write points (`turn/end` and session
/// disposal) always fire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Committed events per session that force a durable checkpoint write
    /// between mandatory points.
    pub write_every_events: u64,
    /// Longest time (milliseconds) a dirty checkpoint may stay unwritten
    /// between mandatory points.
    pub write_interval_ms: u64,
}

/// Per-session write-behind bookkeeping (live sessions only; dropped at
/// retire).
struct DirtyState {
    /// Committed events since the last durable write.
    pending: u64,
    /// Interval trigger armed at the first dirty event after a clean write.
    timer: Option<tokio::task::AbortHandle>,
}

/// Spawn one detached future with the narrowest working executor (the
/// session/event observers run inside a `block_on` on the caller's task;
/// the fallback covers non-tokio threads).
fn spawn_detached<F>(future: F)
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                future.await;
            });
        }
        Err(_) => {
            std::thread::spawn(move || {
                futures::executor::block_on(future);
            });
        }
    }
}

/// The persisted projection cache service.
pub struct SessionProjectionCache {
    ctx: Context,
    config: Config,
    domain: Arc<Domain>,
    table: Arc<dyn KvTable>,
    persistence: Arc<dyn SessionPersistenceApi>,
    dirty: Mutex<HashMap<usize, DirtyState>>,
}

impl Service for SessionProjectionCache {
    fn service_name(&self) -> &'static str {
        "sessionProjectionCache"
    }
}

impl SessionProjectionCache {
    /// Open the domain, install the write-behind listeners, and publish the
    /// service (TS constructor + `Service.init`).
    pub fn install(
        ctx: &Context,
        config: Config,
        facility: &Arc<DomainFacility>,
        persistence: Arc<dyn SessionPersistenceApi>,
    ) -> Result<Arc<Self>, String> {
        if config.write_every_events == 0 || config.write_interval_ms == 0 {
            return Err(
                "session-projection-cache: writeEveryEvents and writeIntervalMs must be positive"
                    .to_string(),
            );
        }
        let domain = futures::executor::block_on(facility.open(&projection_cache_domain_spec()))?;
        let table = domain.table("sessions");
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            config,
            domain: domain.clone(),
            table,
            persistence,
            dirty: Mutex::new(HashMap::new()),
        });
        ctx.register_service(service.clone());

        // Close the domain with the plugin (TS `sessionProjectionCache.domainClose`).
        let domain_for_dispose = domain.clone();
        let _ = ctx.effect(
            "sessionProjectionCache.domainClose",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let domain = domain_for_dispose.clone();
                    Box::pin(async move {
                        domain.close().await;
                    })
                }))
            }),
        );

        // session/event: turn/end is a mandatory point; every other
        // committed event advances the dirty counter and arms the interval
        // trigger once.
        let event_service = service.clone();
        let event_listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = downcast::<SessionEvent>(&args[1])
                .expect("event arg")
                .clone();
            let service = event_service.clone();
            Box::pin(async move {
                if event.type_ == "turn/end" {
                    service.spawn_flush(&session, "turn/end");
                    return None;
                }
                let mut dirty = service.dirty.lock();
                let entry = dirty
                    .entry(session.identity())
                    .or_insert_with(|| DirtyState {
                        pending: 0,
                        timer: None,
                    });
                entry.pending += 1;
                if entry.pending >= service.config.write_every_events {
                    drop(dirty);
                    service.spawn_flush(&session, "count threshold");
                    return None;
                }
                if entry.timer.is_none() {
                    let interval = service.config.write_interval_ms;
                    let session_for_timer = session.clone();
                    let service_for_timer = service.clone();
                    let task = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(interval)).await;
                        service_for_timer.spawn_flush(&session_for_timer, "interval");
                    });
                    entry.timer = Some(task.abort_handle());
                }
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/event",
            event_listener,
            EventOptions::default(),
        ));

        // Detach (the live-to-cold moment): the second mandatory point.
        let disposed_service = service.clone();
        let disposed_listener: Arc<Listener> =
            Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                let session = downcast::<Session>(&args[0]).expect("session arg").clone();
                let service = disposed_service.clone();
                Box::pin(async move {
                    service.spawn_flush(&session, "detach");
                    service.mark_clean(&session);
                    service.dirty.lock().remove(&session.identity());
                    None
                })
            });
        let _ = futures::executor::block_on(ctx.on(
            "session/disposed",
            disposed_listener,
            EventOptions::default(),
        ));

        // Clear pending timers with the plugin (their sessions outlive the
        // cache).
        let timers_service = service.clone();
        let _ = ctx.effect(
            "sessionProjectionCache.timers",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let service = timers_service.clone();
                    Box::pin(async move {
                        for state in service.dirty.lock().values_mut() {
                            if let Some(timer) = state.timer.take() {
                                timer.abort();
                            }
                        }
                        service.dirty.lock().clear();
                    })
                }))
            }),
        );

        Ok(service)
    }

    fn registry(&self) -> Arc<SessionProjectionRegistry> {
        self.ctx
            .get_typed::<Arc<SessionProjectionRegistry>>("sessionProjections", false)
            .expect("sessionProjections service required")
            .as_ref()
            .clone()
    }

    /// The stored record for one session, accepted only when its bound log
    /// identity matches `expected` (TS `recordFor`).
    fn record_for(
        &self,
        id: &SessionId,
        expected: &CheckpointIdentity,
    ) -> Option<CheckpointRecord> {
        let record = self.table.get(id.as_str())?;
        let record: CheckpointRecord =
            serde_json::from_value(record).expect("stored checkpoint record shape");
        if identity_matches(&record.identity, expected) {
            Some(record)
        } else {
            None
        }
    }

    /// The zero-I/O listing read (TS `cachedSnapshot`).
    pub fn cached_snapshot(&self, meta: &SessionHeader) -> Option<ProjectionSnapshot> {
        let record = self.record_for(&meta.id, &identity_of(meta))?;
        let rows = rows_of(Some(&record));
        let values = self.registry().view_checkpoint(&rows);
        if values.is_empty() {
            return None;
        }
        let as_of_seq = rows
            .values()
            .map(|row| row.seq)
            .min()
            .expect("non-empty rows");
        Some(ProjectionSnapshot { as_of_seq, values })
    }

    /// Durably checkpoint one live session NOW (TS `write`). NOT fail-soft
    /// — callers on the fail-soft paths contain it.
    pub async fn write(&self, session: &Session) -> Result<(), String> {
        let rows = self.checkpoint_for_write(session);
        self.write_checkpoint(session, &rows).await
    }

    /// Capture the checkpoint cut and reset its dirty window synchronously.
    /// TS async functions execute this prefix before their first `await`; the
    /// detached Rust path must do the same before yielding to the executor.
    fn checkpoint_for_write(&self, session: &Session) -> ProjectionCheckpoint {
        let rows = self.registry().checkpoint(session);
        self.mark_clean(session);
        rows
    }

    /// Persist a checkpoint whose cut and dirty-window reset already happened.
    async fn write_checkpoint(
        &self,
        session: &Session,
        rows: &ProjectionCheckpoint,
    ) -> Result<(), String> {
        // Durability barrier: the checkpoint cut was taken above, so
        // flushing AFTER it guarantees every event inside the cut is
        // durably logged before the cache row lands.
        let store = self
            .ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|store| store.as_ref().clone());
        if let Some(store) = store
            && store
                .get(session.id())
                .is_some_and(|live| live.ptr_eq(session))
        {
            store.flush(session).await?;
        }
        self.put(session.id(), &identity_of(session.header()), rows)
            .await
    }

    /// Cold-read one persisted session's projections with zero full-log
    /// load (TS `coldSnapshot`).
    pub async fn cold_snapshot(&self, id: &SessionId) -> Result<ProjectionSnapshot, String> {
        let registry = self.registry();
        let record: Option<CheckpointRecord> = self
            .table
            .get(id.as_str())
            .map(|raw| serde_json::from_value(raw).expect("stored checkpoint record shape"));
        let mut cached = rows_of(record.as_ref());
        let rail_needs_seed = cached
            .get(dsh_session_title::USER_MESSAGE_RAIL_KEY)
            .is_none_or(|row| row.ver != dsh_session_title::USER_MESSAGE_RAIL_STATE_VERSION);
        let mut preloaded = None;
        if rail_needs_seed {
            let whole = self.persistence.read_from(id, 0).await?;
            if record.as_ref().is_some_and(|record| {
                !identity_matches(&record.identity, &identity_of(&whole.meta))
            }) {
                cached.clear();
            }
            cached.insert(
                dsh_session_title::USER_MESSAGE_RAIL_KEY.to_string(),
                dsh_session_projection::ProjectionCheckpointRow {
                    ver: dsh_session_title::USER_MESSAGE_RAIL_STATE_VERSION,
                    seq: whole
                        .events
                        .last()
                        .map(|event| event.seq as i64)
                        .unwrap_or(-1),
                    val: dsh_session_title::user_message_rail_rows(&whole.events),
                },
            );
            preloaded = Some(whole);
        }
        let floor = registry.restore_floor(&cached);
        if floor.is_none() {
            // No unit registered: nothing to fold, but the not-found
            // contract must hold in this topology too.
            let probe = self.persistence.read_from(id, 0).await?;
            return Ok(ProjectionSnapshot {
                as_of_seq: probe
                    .events
                    .last()
                    .map(|event| event.seq as i64)
                    .unwrap_or(-1),
                values: serde_json::Map::new(),
            });
        }
        let floor = floor.expect("checked");
        let tail = match preloaded {
            Some(whole) if floor == 0 => whole,
            _ => self.persistence.read_from(id, floor as u64).await?,
        };
        let related = rail_needs_seed
            || record
                .as_ref()
                .is_none_or(|record| identity_matches(&record.identity, &identity_of(&tail.meta)));
        let restored = match (related, registry.restore(&cached, &tail.events, floor)) {
            (true, Ok(restored)) => restored,
            // An unrelated record, or a row overreaching the stored log end
            // (or predating the floor): one full fresh read.
            _ => {
                let whole = self.persistence.read_from(id, 0).await?;
                registry
                    .restore(&ProjectionCheckpoint::new(), &whole.events, 0)
                    .expect("base-0 restore never throws")
            }
        };
        let (snapshot, checkpoint) = restored;
        self.put_soft(
            id,
            &identity_of(&tail.meta),
            &checkpoint,
            "cold-read write-back",
        )
        .await;
        Ok(snapshot)
    }

    /// One fail-soft durable checkpoint (TS `flushSoft`).
    fn spawn_flush(self: &Arc<Self>, session: &Session, trigger: &'static str) {
        let rows = self.checkpoint_for_write(session);
        let service = self.clone();
        let session = session.clone();
        spawn_detached(async move {
            match service.write_checkpoint(&session, &rows).await {
                Ok(_) => {}
                Err(error) => {
                    service.ctx.named_logger(Some("session-projection-cache")).warn(vec![arc(
                        format!(
                            "session projection cache: {trigger} write for \"{}\" failed (cache stays stale): {error}",
                            session.id()
                        ),
                    )]);
                }
            }
        });
    }

    /// Reset one session's dirty bookkeeping (its checkpoint is being
    /// written).
    fn mark_clean(&self, session: &Session) {
        let mut dirty = self.dirty.lock();
        let Some(state) = dirty.get_mut(&session.identity()) else {
            return;
        };
        state.pending = 0;
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
    }

    /// Replace one session's stored record with its log identity and a
    /// detached snapshot of `rows` (TS `put`).
    async fn put(
        &self,
        id: &SessionId,
        identity: &CheckpointIdentity,
        rows: &ProjectionCheckpoint,
    ) -> Result<(), String> {
        let record = JsonValue::Object(serde_json::Map::from_iter([
            (
                "identity".to_string(),
                serde_json::to_value(identity).expect("identity serializes"),
            ),
            (
                "rows".to_string(),
                serde_json::to_value(rows).expect("checkpoint serializes"),
            ),
        ]));
        self.table.put(id.as_str(), record).await
    }

    /// Fail-soft [`put`] (TS `putSoft`).
    async fn put_soft(
        &self,
        id: &SessionId,
        identity: &CheckpointIdentity,
        rows: &ProjectionCheckpoint,
        what: &'static str,
    ) {
        if let Err(error) = self.put(id, identity, rows).await {
            self.ctx.named_logger(Some("session-projection-cache")).warn(vec![arc(
                format!(
                    "session projection cache: {what} for \"{}\" failed (cache stays stale): {error}",
                    id
                ),
            )]);
        }
    }

    /// The domain-close disposer (kept for the plugin path symmetry).
    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }
}

/// Project a header onto the identity fields a record is bound to (TS
/// `identityOf`).
pub fn identity_of(header: &SessionHeader) -> CheckpointIdentity {
    CheckpointIdentity {
        created_at: header.created_at,
        cwd: header.cwd.clone(),
    }
}

/// Whether a stored record's bound identity names the caller's lifecycle
/// (TS `identityMatches`).
pub fn identity_matches(stored: &CheckpointIdentity, expected: &CheckpointIdentity) -> bool {
    stored.created_at == expected.created_at && stored.cwd == expected.cwd
}

/// The no-op companion installer (TS `invariant.ts`).
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-session-projection-cache";

/// The Cordis plugin form of the service (`name =
/// "session-projection-cache"`, TS `inject`). The facility and persistence
/// handles are explicit constructor parameters until the storage hub lands.
pub struct SessionProjectionCachePlugin {
    pub config: Config,
    pub facility: Arc<DomainFacility>,
    pub persistence: Arc<dyn SessionPersistenceApi>,
}

#[async_trait::async_trait]
impl cordis::Plugin for SessionProjectionCachePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT.iter().copied())
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), cordis::PluginError> {
        SessionProjectionCache::install(ctx, self.config, &self.facility, self.persistence.clone())
            .map(|_| ())
            .map_err(|error| cordis::PluginError::new(arc(error)))
    }
}

/// Cordis plugin name (TS `SessionProjectionCache` as a plugin).
pub const NAME: &str = "session-projection-cache";

/// Required services (TS `static inject`).
pub const INJECT: [&str; 4] = [
    "storageDomain",
    "sessionProjections",
    "sessionPersistence",
    "sessions",
];
