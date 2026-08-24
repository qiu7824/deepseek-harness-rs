//! Session-projection registry: the state-driven projection drive. Rust port
//! of `packages/session/session-projection/src/index.ts`.
//!
//! Whole-value event rule (load-bearing): a state-carrying log event MUST
//! carry the complete post-change state, never a bare delta — it keeps every
//! unit's transition trivially cheap and every served value self-describing.
//!
//! # Deviations
//!
//! - The TS `WeakMap<Session, UnitCell>` per-session cells are keyed by the
//!   [`dsh_session::Session::identity`] pointer instead; cells for disposed
//!   sessions are dropped through a `session/disposed` listener (the WeakMap
//!   frees them with the session).
//! - Unit `state` must be plain JSON (`Arc<serde_json::Value>`); `checkpoint`
//!   detaches by deep JSON clone, and a non-JSON state fails loudly where
//!   TS `structuredClone` would have accepted other cloneable shapes.
//! - `stateVersion` is `u64` (the TS non-integer rejection is
//!   inexpressible); `register` surfaces its synchronous throws as `Err`.
//! - `onChanged` listeners are keyed by a fresh registration id (TS uses
//!   identity in a `Set`), so adding the same closure twice notifies twice
//!   where TS would deduplicate it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::{
    ArcValue, Context, Disposer, EventOptions, Listener, Service, arc, downcast, make_disposer,
};
use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub use crate::types::ProjectionValue;

pub type ProjectionSchema = Arc<dyn Fn(&ArcValue) -> Result<ProjectionValue, String> + Send + Sync>;
pub type ProjectionApply = Arc<dyn Fn(&ArcValue, &SessionEvent) -> ArcValue + Send + Sync>;

/// One domain's state-driven computation unit (TS `ProjectionDefinition`).
/// All three functions MUST be synchronous and `state` MUST be plain JSON
/// (the persisted-cache precondition). An unchanged state reference
/// (`Arc::ptr_eq`, the TS `Object.is` equivalent) produces zero downstream
/// work.
pub struct ProjectionDefinition {
    /// The projection key this unit owns.
    pub key: String,
    /// Validates the wire payload (`view` output) before it leaves the host;
    /// returns the JSON snapshot served to consumers.
    pub schema: ProjectionSchema,
    /// State for the empty log.
    pub init: Arc<dyn Fn() -> ArcValue + Send + Sync>,
    /// Pure transition: previous state + one committed event → next state.
    /// Return the SAME `Arc` when the event is not the unit's.
    pub apply: ProjectionApply,
    /// State → wire payload (the read-side projection).
    pub view: Arc<dyn Fn(&ArcValue) -> ArcValue + Send + Sync>,
    /// Persisted-cache invalidation version (non-negative integer).
    pub state_version: u64,
}

/// Change-feed listener: one unit's value changed for one session (TS
/// `ProjectionChangeListener`).
pub type ProjectionChangeListener =
    Arc<dyn Fn(&Session, &str, &ProjectionValue, i64) + Send + Sync>;

/// One consistent read cut over every registered unit for one session (TS
/// `ProjectionSnapshot`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionSnapshot {
    /// Seq of the last event the values reflect; -1 for an empty log.
    pub as_of_seq: i64,
    /// Whole current value per registered key.
    pub values: serde_json::Map<String, ProjectionValue>,
}

/// One unit's checkpoint row (TS `ProjectionCheckpointRow`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionCheckpointRow {
    /// The registering unit's `stateVersion` at fold time.
    pub ver: u64,
    /// Seq of the last event folded into `val`; -1 for the empty log.
    pub seq: i64,
    /// The unit's internal state — plain JSON per the unit contract.
    pub val: ProjectionValue,
}

/// Checkpoint rows keyed by projection key (TS `ProjectionCheckpoint`).
pub type ProjectionCheckpoint = indexmap::IndexMap<String, ProjectionCheckpointRow>;

/// Per-session per-unit watermark cache row (TS `UnitCell`).
struct UnitCell {
    state: ArcValue,
    /// Seq of the last event passed through `apply` (regardless of change).
    observed_seq: i64,
}

/// One live registration: the unit plus its per-session cells.
struct Registration {
    def: ProjectionDefinition,
    cells: Mutex<HashMap<usize, UnitCell>>,
    /// Live registrants sharing this unit; the last one out removes the key.
    refs: usize,
}

/// `ctx.sessionProjections`: the projection unit table and its drive.
pub struct SessionProjectionRegistry {
    registrations: Mutex<HashMap<String, Registration>>,
    listeners: Mutex<Vec<(u64, ProjectionChangeListener)>>,
    next_listener_id: AtomicU64,
}

impl Service for SessionProjectionRegistry {
    fn service_name(&self) -> &'static str {
        "sessionProjections"
    }
}

impl SessionProjectionRegistry {
    /// Create and install the registry as `ctx.sessionProjections`,
    /// subscribing to `session/event` once (TS constructor).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let registry = Arc::new(Self {
            registrations: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: AtomicU64::new(0),
        });
        ctx.register_service(registry.clone());

        // session/event: pass every committed event through every unit.
        let event_registry = Arc::clone(&registry);
        let event_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = downcast::<SessionEvent>(&args[1])
                .expect("event arg")
                .clone();
            let registry = Arc::clone(&event_registry);
            Box::pin(async move {
                registry.drive(&session, &event);
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/event",
            event_listener,
            EventOptions::default(),
        ));

        // session/disposed: drop the session's cells (WeakMap equivalent).
        let disposed_registry = Arc::clone(&registry);
        let disposed_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let registry = Arc::clone(&disposed_registry);
            Box::pin(async move {
                registry.forget_session(&session);
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/disposed",
            disposed_listener,
            EventOptions::default(),
        ));

        registry
    }

    /// Register one domain's unit. The registration is an effect on the
    /// CALLER context's fiber (TS `register`; cordis rebinds `this.ctx` to
    /// the caller, so the caller context is an explicit parameter here).
    /// The synchronous throws become `Err`.
    pub fn register(
        self: &Arc<Self>,
        caller: &Context,
        definition: ProjectionDefinition,
    ) -> Result<Disposer, String> {
        let key = definition.key.clone();
        {
            let mut registrations = self.registrations.lock();
            match registrations.get_mut(&key) {
                None => {
                    registrations.insert(
                        key.clone(),
                        Registration {
                            def: definition,
                            cells: Mutex::new(HashMap::new()),
                            refs: 1,
                        },
                    );
                }
                Some(existing) => {
                    // The one incompatibility this can name: the versioned
                    // contract says the cached state shape differs.
                    if existing.def.state_version != definition.state_version {
                        return Err(format!(
                            "session projection key {} is already registered at stateVersion {}; refusing to share it with stateVersion {}",
                            serde_json::to_string(&key).unwrap_or_else(|_| key.clone()),
                            existing.def.state_version,
                            definition.state_version
                        ));
                    }
                    existing.refs += 1;
                }
            }
        }
        let registry = Arc::clone(self);
        let dispose = caller.effect(
            "sessionProjections.register()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let registry = Arc::clone(&registry);
                    let key = key.clone();
                    Box::pin(async move { registry.release(&key) })
                }))
            }),
        );
        Ok(dispose)
    }

    /// Subscribe to the change feed (an effect on the caller context's
    /// fiber; TS `onChanged`).
    pub fn on_changed(
        self: &Arc<Self>,
        caller: &Context,
        listener: ProjectionChangeListener,
    ) -> Disposer {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().push((id, listener));
        let registry = Arc::clone(self);

        (caller.effect(
            "sessionProjections.onChanged()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let registry = Arc::clone(&registry);
                    Box::pin(async move {
                        registry.listeners.lock().retain(|(entry, _)| *entry != id);
                    })
                }))
            }),
        )) as _
    }

    /// One consistent cut over every registered unit for one session (TS
    /// `snapshot`). Fully synchronous; schema failures panic (the TS throw).
    pub fn snapshot(&self, session: &Session) -> ProjectionSnapshot {
        let mut values = serde_json::Map::new();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let cell = cell_for(registration, session);
            let parsed = (registration.def.schema)(&(registration.def.view)(&cell.state))
                .expect("session projection view violated its schema");
            values.insert(registration.def.key.clone(), parsed);
        }
        ProjectionSnapshot {
            as_of_seq: session.seq() as i64 - 1,
            values,
        }
    }

    /// State-level checkpoint of every registered unit (TS `checkpoint`).
    /// Every `val` is a DETACHED deep clone of the cell state.
    pub fn checkpoint(&self, session: &Session) -> ProjectionCheckpoint {
        let mut rows = ProjectionCheckpoint::new();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let cell = cell_for(registration, session);
            let value: Arc<ProjectionValue> = cordis::downcast_arc(&cell.state)
                .expect("session projection state must be plain JSON");
            rows.insert(
                registration.def.key.clone(),
                ProjectionCheckpointRow {
                    ver: registration.def.state_version,
                    seq: cell.observed_seq,
                    val: value.as_ref().clone(),
                },
            );
        }
        rows
    }

    /// The stored seq a restore tail read must start at (TS `restoreFloor`):
    /// one event BELOW the lowest usable watermark; `0` for missing or
    /// mismatched rows; `None` when no unit is registered.
    pub fn restore_floor(&self, checkpoint: &ProjectionCheckpoint) -> Option<i64> {
        let mut floor: Option<i64> = None;
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let row = checkpoint.get(&registration.def.key);
            let need = match row {
                Some(row) if row.ver == registration.def.state_version => {
                    std::cmp::max(row.seq + 1, 0)
                }
                _ => 0,
            };
            floor = Some(match floor {
                None => need,
                Some(current) => std::cmp::min(current, need),
            });
        }
        floor.map(|floor| std::cmp::max(floor - 1, 0))
    }

    /// View a checkpoint's rows without any log read (TS `viewCheckpoint`).
    pub fn view_checkpoint(
        &self,
        checkpoint: &ProjectionCheckpoint,
    ) -> serde_json::Map<String, ProjectionValue> {
        let mut values = serde_json::Map::new();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let def = &registration.def;
            let Some(row) = checkpoint.get(&def.key) else {
                continue;
            };
            if row.ver != def.state_version {
                continue;
            }
            let parsed = (def.schema)(&(def.view)(&arc(row.val.clone())))
                .expect("session projection view violated its schema");
            values.insert(def.key.clone(), parsed);
        }
        values
    }

    /// Cold read: fold every registered unit over a stored log suffix (TS
    /// `restore`); its synchronous throws become `Err`.
    pub fn restore(
        &self,
        checkpoint: &ProjectionCheckpoint,
        events: &[SessionEvent],
        base_seq: i64,
    ) -> Result<(ProjectionSnapshot, ProjectionCheckpoint), String> {
        let end_seq = events
            .last()
            .map(|event| event.seq as i64)
            .unwrap_or(base_seq - 1);
        let mut values = serde_json::Map::new();
        let mut refreshed = ProjectionCheckpoint::new();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let def = &registration.def;
            let row = checkpoint.get(&def.key);
            let usable = row.is_some_and(|row| {
                row.ver == def.state_version && row.seq >= base_seq - 1 && row.seq <= end_seq
            });
            if !usable && base_seq > 0 {
                return Err(format!(
                    "session projection {} cannot restore from seq {base_seq}: its checkpoint row is missing, version-mismatched, or beyond the supplied log end; re-read from seq 0",
                    serde_json::to_string(&def.key).unwrap_or_else(|_| def.key.clone())
                ));
            }
            let row = row.filter(|_row| usable);
            let mut state: ArcValue = match row {
                Some(row) => arc(row.val.clone()),
                None => (def.init)(),
            };
            let from = row.map(|row| row.seq).unwrap_or(base_seq - 1);
            for event in events {
                if event.seq as i64 > from {
                    state = (def.apply)(&state, event);
                }
            }
            let parsed = (def.schema)(&(def.view)(&state))
                .expect("session projection view violated its schema");
            values.insert(def.key.clone(), parsed);
            let state_value: Arc<ProjectionValue> =
                cordis::downcast_arc(&state).expect("session projection state must be plain JSON");
            refreshed.insert(
                def.key.clone(),
                ProjectionCheckpointRow {
                    ver: def.state_version,
                    seq: end_seq,
                    val: state_value.as_ref().clone(),
                },
            );
        }
        Ok((
            ProjectionSnapshot {
                as_of_seq: end_seq,
                values,
            },
            refreshed,
        ))
    }

    /// The last registrant release removes the key.
    fn release(&self, key: &str) {
        let mut registrations = self.registrations.lock();
        let Some(registration) = registrations.get_mut(key) else {
            return;
        };
        registration.refs -= 1;
        if registration.refs == 0 {
            registrations.remove(key);
        }
    }

    /// Drop one disposed session's cells (the WeakMap frees them with the
    /// session in TS).
    fn forget_session(&self, session: &Session) {
        let identity = session.identity();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            registration.cells.lock().remove(&identity);
        }
    }

    /// Eager drive: pass one committed event through every registered unit;
    /// notify on changed references (TS `drive`).
    fn drive(&self, session: &Session, event: &SessionEvent) {
        let listeners = self.listeners.lock().clone();
        let registrations = self.registrations.lock();
        for registration in registrations.values() {
            let (next, changed) = {
                let mut cells = registration.cells.lock();
                let cell = match cells.get_mut(&session.identity()) {
                    Some(cell) => cell,
                    None => {
                        // Late build mid-stream: fold history before this
                        // event (seq = log index, so the prefix slice is
                        // exact), then take the normal gate.
                        let events = session.events();
                        let prefix = &events[..event.seq as usize];
                        cells.insert(session.identity(), build_cell(&registration.def, prefix));
                        cells
                            .get_mut(&session.identity())
                            .expect("cell just inserted")
                    }
                };
                let next = (registration.def.apply)(&cell.state, event);
                let changed = !Arc::ptr_eq(&next, &cell.state);
                cell.state = next;
                cell.observed_seq = event.seq as i64;
                (cell.state.clone(), changed)
            };
            if changed && !listeners.is_empty() {
                let value = (registration.def.schema)(&(registration.def.view)(&next))
                    .expect("session projection view violated its schema");
                for (_, listener) in &listeners {
                    listener(session, &registration.def.key, &value, event.seq as i64);
                }
            }
        }
    }
}

/// Fold one unit from init over `events` (TS `buildCell`).
fn build_cell(def: &ProjectionDefinition, events: &[SessionEvent]) -> UnitCell {
    let mut state = (def.init)();
    for event in events {
        state = (def.apply)(&state, event);
    }
    UnitCell {
        state,
        observed_seq: events.last().map(|event| event.seq as i64).unwrap_or(-1),
    }
}

/// Read (or lazily build, folding the full in-memory log) one unit's cell
/// (TS `cellFor`).
fn cell_for(registration: &Registration, session: &Session) -> UnitCell {
    let identity = session.identity();
    if let Some(cell) = registration.cells.lock().get(&identity) {
        return UnitCell {
            state: cell.state.clone(),
            observed_seq: cell.observed_seq,
        };
    }
    let events = session.events();
    let built = build_cell(&registration.def, &events);
    registration.cells.lock().insert(
        identity,
        UnitCell {
            state: built.state.clone(),
            observed_seq: built.observed_seq,
        },
    );
    built
}

impl Default for ProjectionSnapshot {
    fn default() -> Self {
        Self {
            as_of_seq: -1,
            values: serde_json::Map::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::SessionStore;

    fn marks_schema(value: &ArcValue) -> Result<ProjectionValue, String> {
        let value: &ProjectionValue =
            cordis::downcast(value).ok_or_else(|| "view must produce a JSON value".to_string())?;
        let marks = value
            .get("marks")
            .and_then(|marks| marks.as_array())
            .ok_or_else(|| "marks array required".to_string())?;
        for entry in marks {
            if !entry.is_string() {
                return Err("marks entries must be strings".to_string());
            }
        }
        if !value.is_object() || value.as_object().map(|object| object.len()).unwrap_or(0) != 1 {
            return Err("unexpected extra keys".to_string());
        }
        Ok(value.clone())
    }

    /// Whole-value unit: latest `test/mark` event wins; unrelated events
    /// return the same reference. State is plain JSON per the unit contract
    /// (the TS `{ marks: string[] } | null`).
    fn marks_unit() -> ProjectionDefinition {
        ProjectionDefinition {
            key: "test/marks".to_string(),
            schema: Arc::new(marks_schema),
            init: Arc::new(|| arc(serde_json::Value::Null)),
            apply: Arc::new(|state: &ArcValue, event: &SessionEvent| {
                if event.type_ == "test/mark" {
                    let marks: Vec<String> = serde_json::from_value(
                        event.data.get("marks").cloned().unwrap_or_default(),
                    )
                    .unwrap_or_default();
                    arc(serde_json::json!({"marks": marks}))
                } else {
                    state.clone()
                }
            }),
            view: Arc::new(|state: &ArcValue| {
                let value: &serde_json::Value = cordis::downcast(state).expect("marks state");
                if value.is_null() {
                    arc(serde_json::json!({"marks": []}))
                } else {
                    Arc::clone(state)
                }
            }),
            state_version: 1,
        }
    }

    /// Counting unit over every event — state changes on each apply. State
    /// is plain JSON (the TS `number`).
    fn count_unit() -> ProjectionDefinition {
        ProjectionDefinition {
            key: "test/count".to_string(),
            schema: Arc::new(|value: &ArcValue| {
                let value: &ProjectionValue = cordis::downcast(value)
                    .ok_or_else(|| "view must produce a JSON value".to_string())?;
                let count = value
                    .as_i64()
                    .ok_or_else(|| "count must be an integer".to_string())?;
                if count < 0 {
                    return Err("count must be non-negative".to_string());
                }
                Ok(value.clone())
            }),
            init: Arc::new(|| arc(serde_json::json!(0))),
            apply: Arc::new(|state: &ArcValue, _event: &SessionEvent| {
                let count: &serde_json::Value = cordis::downcast(state).expect("count state");
                arc(serde_json::json!(count.as_i64().expect("count state") + 1))
            }),
            view: Arc::new(|state: &ArcValue| state.clone()),
            state_version: 1,
        }
    }

    async fn harness() -> (Context, Arc<SessionProjectionRegistry>, Session) {
        let ctx = Context::root();
        let store = SessionStore::install(&ctx);
        let registry = SessionProjectionRegistry::install(&ctx);
        let session = store
            .create(
                &ctx,
                None,
                Some(dsh_session::CreateSessionOptions::default()),
            )
            .await
            .unwrap();
        (ctx, registry, session)
    }

    fn mark(session: &Session, marks: &[&str]) {
        let marks: Vec<String> = marks.iter().map(|mark| mark.to_string()).collect();
        session
            .append("test/mark", serde_json::json!({"marks": marks}), None)
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drives_registered_unit_and_snapshots_current_value() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        mark(&session, &["a"]);
        mark(&session, &["a", "b"]);
        let snapshot = registry.snapshot(&session);
        assert_eq!(
            snapshot.values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["a", "b"]}))
        );
        assert_eq!(snapshot.as_of_seq, session.seq() as i64 - 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn builds_cell_lazily_for_unit_registered_after_events() {
        let (ctx, registry, session) = harness().await;
        mark(&session, &["pre-registration"]);
        registry.register(&ctx, marks_unit()).unwrap();
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["pre-registration"]}))
        );
        // The lazily-built cell then continues on the live drive path.
        mark(&session, &["after"]);
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["after"]}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn serves_init_state_and_as_of_seq_minus_one_for_empty_log() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        let snapshot = registry.snapshot(&session);
        assert_eq!(snapshot.as_of_seq, -1);
        assert_eq!(
            snapshot.values.get("test/marks"),
            Some(&serde_json::json!({"marks": []}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn notifies_on_changed_and_skips_same_reference_applies() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        let seen: Arc<Mutex<Vec<(String, ProjectionValue, i64)>>> =
            Arc::new(Mutex::new(Vec::new()));
        {
            let seen = Arc::clone(&seen);
            registry.on_changed(
                &ctx,
                Arc::new(
                    move |_changed_session: &Session,
                          key: &str,
                          value: &ProjectionValue,
                          seq: i64| {
                        seen.lock().push((key.to_string(), value.clone(), seq));
                    },
                ),
            );
        }
        mark(&session, &["a"]);
        // Non-matching event: apply returns the same reference — no
        // notification.
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        assert_eq!(
            seen.lock().clone(),
            vec![(
                "test/marks".to_string(),
                serde_json::json!({"marks": ["a"]}),
                0
            )]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drives_independently_per_session() {
        let (ctx, registry, session) = harness().await;
        let store = ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .expect("sessions");
        let other = store
            .create(
                &ctx,
                None,
                Some(dsh_session::CreateSessionOptions::default()),
            )
            .await
            .unwrap();
        registry.register(&ctx, marks_unit()).unwrap();
        mark(&session, &["one"]);
        mark(&other, &["two"]);
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["one"]}))
        );
        assert_eq!(
            registry.snapshot(&other).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["two"]}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_every_registered_unit_and_only_notifies_changing_ones() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        registry.register(&ctx, count_unit()).unwrap();
        let changed_keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let changed_keys = Arc::clone(&changed_keys);
            registry.on_changed(
                &ctx,
                Arc::new(
                    move |_session: &Session, key: &str, _value: &ProjectionValue, _seq: i64| {
                        changed_keys.lock().push(key.to_string());
                    },
                ),
            );
        }
        session
            .append("turn/start", serde_json::json!({"turn": 1}), None)
            .unwrap();
        assert_eq!(changed_keys.lock().clone(), vec!["test/count".to_string()]);
        let snapshot = registry.snapshot(&session);
        assert_eq!(
            snapshot.values.get("test/count"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            snapshot.values.get("test/marks"),
            Some(&serde_json::json!({"marks": []}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shares_one_unit_between_registrants_and_keeps_until_last_release() {
        let (ctx, registry, session) = harness().await;
        let first = registry.register(&ctx, marks_unit()).unwrap();
        let second = registry.register(&ctx, marks_unit()).unwrap();
        mark(&session, &["kept"]);

        first().await;
        // One session ending must not strip the projection from other
        // sessions (the ref-count regression).
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["kept"]}))
        );
        second().await;
        assert_eq!(registry.snapshot(&session).values, serde_json::Map::new());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_state_version_change_and_rejects_invalid_versions() {
        let (ctx, registry, _session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        let mut changed = marks_unit();
        changed.state_version = 9;
        let error = registry
            .register(&ctx, changed)
            .err()
            .expect("register must fail");
        assert!(
            error.contains(
                "already registered at stateVersion 1; refusing to share it with stateVersion 9"
            ),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn register_disposer_removes_key_and_frees_for_reregistration() {
        let (ctx, registry, session) = harness().await;
        let dispose = registry.register(&ctx, marks_unit()).unwrap();
        mark(&session, &["cached"]);
        dispose().await;
        assert_eq!(registry.snapshot(&session).values, serde_json::Map::new());
        registry.register(&ctx, marks_unit()).unwrap();
        // Fresh registration rebuilds from the log, not from a stale cell.
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["cached"]}))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checkpoints_every_unit_with_version_and_watermark() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        let mut counting = count_unit();
        counting.state_version = 7;
        registry.register(&ctx, counting).unwrap();
        mark(&session, &["a"]);
        let rows = registry.checkpoint(&session);
        assert_eq!(rows.get("test/marks").unwrap().ver, 1);
        assert_eq!(rows.get("test/marks").unwrap().seq, 0);
        assert_eq!(
            rows.get("test/marks").unwrap().val,
            serde_json::json!({"marks": ["a"]})
        );
        assert_eq!(rows.get("test/count").unwrap().ver, 7);
        assert_eq!(rows.get("test/count").unwrap().val, serde_json::json!(1));
        // Empty log: init-derived state at watermark -1.
        let (ctx2, registry2, _) = harness().await;
        let store = ctx2
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .expect("sessions");
        let fresh = store
            .create(
                &ctx2,
                None,
                Some(dsh_session::CreateSessionOptions::default()),
            )
            .await
            .unwrap();
        registry2.register(&ctx2, marks_unit()).unwrap();
        let empty = registry2.checkpoint(&fresh);
        assert_eq!(empty.get("test/marks").unwrap().seq, -1);
        assert_eq!(
            empty.get("test/marks").unwrap().val,
            serde_json::Value::Null
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checkpoint_states_are_detached_clones() {
        let (ctx, registry, session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        mark(&session, &["a"]);
        let rows = registry.checkpoint(&session);
        // A hostile consumer mutates the handed-out state.
        let mut injected = rows.get("test/marks").unwrap().val.clone();
        injected["marks"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("INJECTED"));
        // The registry's authoritative cell is untouched.
        assert_eq!(
            registry.snapshot(&session).values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["a"]}))
        );
        assert_eq!(
            registry.checkpoint(&session).get("test/marks").unwrap().val,
            serde_json::json!({"marks": ["a"]})
        );
    }

    fn checkpoint_row(key: &str, ver: u64, seq: i64, val: ProjectionValue) -> ProjectionCheckpoint {
        let mut rows = ProjectionCheckpoint::new();
        rows.insert(key.to_string(), ProjectionCheckpointRow { ver, seq, val });
        rows
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restore_floor_anchors_one_below_lowest_usable_watermark() {
        let (ctx, registry, _session) = harness().await;
        assert_eq!(
            registry.restore_floor(&ProjectionCheckpoint::new()),
            None,
            "no unit registered"
        );
        registry.register(&ctx, marks_unit()).unwrap();
        registry.register(&ctx, count_unit()).unwrap();
        assert_eq!(
            registry.restore_floor(&ProjectionCheckpoint::new()),
            Some(0)
        );
        let mut rows = ProjectionCheckpoint::new();
        rows.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 10,
                val: serde_json::json!({"marks": []}),
            },
        );
        rows.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 5,
                val: serde_json::json!(6),
            },
        );
        assert_eq!(registry.restore_floor(&rows), Some(5));
        let mut mismatched = ProjectionCheckpoint::new();
        mismatched.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 2,
                seq: 10,
                val: serde_json::json!({"marks": []}),
            },
        );
        mismatched.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 5,
                val: serde_json::json!(6),
            },
        );
        assert_eq!(registry.restore_floor(&mismatched), Some(0));
        let mut fresh = ProjectionCheckpoint::new();
        fresh.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: -1,
                val: serde_json::Value::Null,
            },
        );
        fresh.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: -1,
                val: serde_json::json!(0),
            },
        );
        assert_eq!(registry.restore_floor(&fresh), Some(0));
    }

    fn event(type_: &str, seq: u64, time: i64, data: ProjectionValue) -> SessionEvent {
        SessionEvent {
            type_: type_.to_string(),
            seq,
            time,
            data,
            ignorable: None,
            surface_op: None,
            source_event_seqs: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restore_folds_tail_and_refolds_mismatched_rows_from_init() {
        let (ctx, registry, _session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        registry.register(&ctx, count_unit()).unwrap();
        let tail = vec![
            event("test/mark", 3, 3, serde_json::json!({"marks": ["new"]})),
            event(
                "turn/end",
                4,
                4,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
        ];
        // marks row usable; count row mismatched — a mismatch with
        // baseSeq > 0 cannot silently refold: it throws for a re-read.
        let mut rows = ProjectionCheckpoint::new();
        rows.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 2,
                val: serde_json::json!({"marks": ["old"]}),
            },
        );
        rows.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 99,
                seq: 2,
                val: serde_json::json!(3),
            },
        );
        let error = registry.restore(&rows, &tail, 3).unwrap_err();
        assert!(error.contains("re-read from seq 0"), "{error}");
        // The full-log re-read (baseSeq 0) refolds the mismatched key from init.
        let full = vec![
            event("turn/start", 0, 0, serde_json::json!({"turn": 1})),
            event("test/mark", 1, 1, serde_json::json!({"marks": ["old"]})),
            event(
                "test/mark",
                2,
                2,
                serde_json::json!({"marks": ["old", "2"]}),
            ),
            tail[0].clone(),
            tail[1].clone(),
        ];
        let (snapshot, refreshed) = registry.restore(&rows, &full, 0).unwrap();
        assert_eq!(snapshot.as_of_seq, 4);
        assert_eq!(
            snapshot.values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["new"]}))
        );
        assert_eq!(
            snapshot.values.get("test/count"),
            Some(&serde_json::json!(5))
        );
        assert_eq!(refreshed.get("test/marks").unwrap().seq, 4);
        assert_eq!(
            refreshed.get("test/marks").unwrap().val,
            serde_json::json!({"marks": ["new"]})
        );
        assert_eq!(
            refreshed.get("test/count").unwrap().val,
            serde_json::json!(5)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restore_over_suffix_folds_only_past_row_watermarks() {
        let (ctx, registry, _session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        registry.register(&ctx, count_unit()).unwrap();
        let mut rows = ProjectionCheckpoint::new();
        rows.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 4,
                val: serde_json::json!({"marks": ["done"]}),
            },
        );
        rows.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 2,
                val: serde_json::json!(3),
            },
        );
        let tail = vec![
            event("turn/start", 3, 3, serde_json::json!({"turn": 2})),
            event(
                "turn/end",
                4,
                4,
                serde_json::json!({"turn": 2, "reason": {"kind": "completed"}}),
            ),
        ];
        let (snapshot, _) = registry.restore(&rows, &tail, 3).unwrap();
        assert_eq!(snapshot.as_of_seq, 4);
        assert_eq!(
            snapshot.values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["done"]}))
        );
        assert_eq!(
            snapshot.values.get("test/count"),
            Some(&serde_json::json!(5))
        );

        // Empty tail (checkpoint is current): the cut sits at baseSeq - 1.
        let mut current_rows = ProjectionCheckpoint::new();
        current_rows.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 4,
                val: serde_json::json!({"marks": ["done"]}),
            },
        );
        current_rows.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 4,
                val: serde_json::json!(5),
            },
        );
        let (current, _) = registry.restore(&current_rows, &[], 5).unwrap();
        assert_eq!(current.as_of_seq, 4);
        assert_eq!(
            current.values.get("test/count"),
            Some(&serde_json::json!(5))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn view_checkpoint_serves_matching_rows_and_skips_mismatched() {
        let (ctx, registry, _session) = harness().await;
        registry.register(&ctx, marks_unit()).unwrap();
        registry.register(&ctx, count_unit()).unwrap();
        let mut rows = ProjectionCheckpoint::new();
        rows.insert(
            "test/marks".to_string(),
            ProjectionCheckpointRow {
                ver: 1,
                seq: 4,
                val: serde_json::json!({"marks": ["stored"]}),
            },
        );
        rows.insert(
            "test/count".to_string(),
            ProjectionCheckpointRow {
                ver: 99,
                seq: 4,
                val: serde_json::json!(5),
            },
        );
        let values = registry.view_checkpoint(&rows);
        assert_eq!(
            values.get("test/marks"),
            Some(&serde_json::json!({"marks": ["stored"]}))
        );
        assert!(!values.contains_key("test/count"));
        assert_eq!(
            registry.view_checkpoint(&ProjectionCheckpoint::new()),
            serde_json::Map::new()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restore_rejects_row_claiming_events_past_log_end() {
        let (ctx, registry, _session) = harness().await;
        registry.register(&ctx, count_unit()).unwrap();
        let rows = checkpoint_row("test/count", 1, 9, serde_json::json!(10));
        let floor = registry.restore_floor(&rows).unwrap();
        assert_eq!(floor, 9);
        // An intact log serves the anchor event and the checkpoint stands.
        let anchor = event(
            "turn/end",
            9,
            9,
            serde_json::json!({"turn": 2, "reason": {"kind": "completed"}}),
        );
        let (snapshot, _) = registry.restore(&rows, &[anchor], 9).unwrap();
        assert_eq!(
            snapshot.values.get("test/count"),
            Some(&serde_json::json!(10))
        );
        // A shrunk log returns an empty tail: the row overreaches the proven
        // end and a tail read cannot fix this key.
        let error = registry.restore(&rows, &[], 9).unwrap_err();
        assert!(error.contains("re-read from seq 0"), "{error}");
        // The full re-read discards the overreaching row and refolds.
        let events = vec![
            event("turn/start", 0, 0, serde_json::json!({"turn": 1})),
            event(
                "turn/end",
                1,
                1,
                serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
            ),
        ];
        let (snapshot, _) = registry.restore(&rows, &events, 0).unwrap();
        assert_eq!(snapshot.as_of_seq, 1);
        assert_eq!(
            snapshot.values.get("test/count"),
            Some(&serde_json::json!(2))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fails_loud_when_view_violates_its_schema() {
        let (ctx, registry, session) = harness().await;
        registry
            .register(
                &ctx,
                ProjectionDefinition {
                    key: "test/marks".to_string(),
                    schema: Arc::new(marks_schema),
                    init: Arc::new(|| arc(serde_json::Value::Null)),
                    apply: Arc::new(|state: &ArcValue, _event: &SessionEvent| state.clone()),
                    // A unit whose view output cannot satisfy its schema fails
                    // at the boundary parse (the TS async-view analogue).
                    view: Arc::new(|_state: &ArcValue| arc(serde_json::json!("not-an-object"))),
                    state_version: 1,
                },
            )
            .unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry.snapshot(&session);
        }));
        assert!(outcome.is_err(), "schema-violating view must panic");
    }
}
