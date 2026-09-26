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
use dsh_session::{Session, SessionEvent, SessionHeader};
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
#[derive(Clone)]
pub struct ProjectionDefinition {
    /// The projection key this unit owns.
    pub key: String,
    /// Validates the wire payload (`view` output) before it leaves the host;
    /// returns the JSON snapshot served to consumers.
    pub schema: ProjectionSchema,
    /// State for the empty log.
    pub init: Arc<dyn Fn(&SessionHeader) -> ArcValue + Send + Sync>,
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

#[path = "stream_restore.rs"]
mod stream_restore;
pub use stream_restore::ProjectionStreamRestore;

/// Per-session per-unit watermark cache row (TS `UnitCell`).
#[derive(Clone)]
struct UnitCell {
    state: ArcValue,
    /// Seq of the last event passed through `apply` (regardless of change).
    observed_seq: i64,
    /// Raw view identity last observed by the change feed. `None` means no
    /// comparable baseline exists (first listener or an unobserved change).
    observed_view: Option<ArcValue>,
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
        let event_listener: Arc<Listener> = Arc::new(move |ctx, args| {
            let ctx = ctx.clone();
            let session = downcast::<Session>(&args[0]).expect("session arg").clone();
            let event = args[1].clone();
            let registry = Arc::clone(&event_registry);
            Box::pin(async move {
                if let Err(error) = registry.drive(
                    &session,
                    downcast::<SessionEvent>(&event).expect("event arg"),
                ) {
                    registry.forget_session(&session);
                    ctx.named_logger(Some("session-projection"))
                        .warn(vec![arc(format!(
                            "session \"{}\" projection history could not be read: {error}",
                            session.id()
                        ))]);
                }
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

    /// The registered projection keys, sorted for deterministic diagnostics.
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.registrations.lock().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Clone only the requested registered units under one registry lock.
    /// Consumers can make bounded summaries without depending on each domain.
    pub fn definitions_for_keys(&self, keys: &[&str]) -> Vec<ProjectionDefinition> {
        let registrations = self.registrations.lock();
        keys.iter()
            .filter_map(|key| registrations.get(*key).map(|row| row.def.clone()))
            .collect()
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
        self.try_snapshot(session)
            .expect("session projection history or view is invalid")
    }

    /// Fallible counterpart for persistent Session readers.
    pub fn try_snapshot(&self, session: &Session) -> Result<ProjectionSnapshot, String> {
        let mut values = serde_json::Map::new();
        let registrations = self.registrations.lock();
        let mut cells = cells_for(&registrations, session, true)?;
        for registration in registrations.values() {
            let cell = cells
                .remove(&registration.def.key)
                .expect("registered cell");
            let parsed = (registration.def.schema)(&(registration.def.view)(&cell.state))?;
            values.insert(registration.def.key.clone(), parsed);
        }
        Ok(ProjectionSnapshot {
            as_of_seq: session.seq().get() as i64 - 1,
            values,
        })
    }

    /// State-level checkpoint of every registered unit (TS `checkpoint`).
    /// Every `val` is a DETACHED deep clone of the cell state.
    pub fn checkpoint(&self, session: &Session) -> ProjectionCheckpoint {
        self.checkpoint_with_retention(session, true)
    }

    /// Capture the final detached state without reviving the strong cell
    /// entries already removed by a session/disposed observer.
    pub fn checkpoint_detached(&self, session: &Session) -> ProjectionCheckpoint {
        self.checkpoint_with_retention(session, false)
    }

    fn checkpoint_with_retention(&self, session: &Session, retain: bool) -> ProjectionCheckpoint {
        self.try_checkpoint_with_retention(session, retain)
            .expect("session projection checkpoint history is invalid")
    }

    pub fn try_checkpoint(&self, session: &Session) -> Result<ProjectionCheckpoint, String> {
        self.try_checkpoint_with_retention(session, true)
    }

    pub fn try_checkpoint_detached(
        &self,
        session: &Session,
    ) -> Result<ProjectionCheckpoint, String> {
        self.try_checkpoint_with_retention(session, false)
    }

    fn try_checkpoint_with_retention(
        &self,
        session: &Session,
        retain: bool,
    ) -> Result<ProjectionCheckpoint, String> {
        let mut rows = ProjectionCheckpoint::new();
        let registrations = self.registrations.lock();
        let mut cells = cells_for(&registrations, session, retain)?;
        for registration in registrations.values() {
            let cell = cells
                .remove(&registration.def.key)
                .expect("registered cell");
            let value: Arc<ProjectionValue> = cordis::downcast_arc(&cell.state)
                .ok_or("session projection state must be plain JSON")?;
            rows.insert(
                registration.def.key.clone(),
                ProjectionCheckpointRow {
                    ver: registration.def.state_version,
                    seq: cell.observed_seq,
                    val: value.as_ref().clone(),
                },
            );
        }
        Ok(rows)
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
        header: &SessionHeader,
        checkpoint: &ProjectionCheckpoint,
        events: &[SessionEvent],
        base_seq: i64,
    ) -> Result<(ProjectionSnapshot, ProjectionCheckpoint), String> {
        let end_seq = events
            .last()
            .map(|event| event.seq.get() as i64)
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
                None => (def.init)(header),
            };
            let from = row.map(|row| row.seq).unwrap_or(base_seq - 1);
            for event in events {
                if event.seq.get() as i64 > from {
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
    fn drive(&self, session: &Session, event: &SessionEvent) -> Result<(), String> {
        let listeners = self.listeners.lock().clone();
        let registrations = self.registrations.lock();
        let mut rebuilt = Vec::new();
        for registration in registrations.values() {
            if !registration.cells.lock().contains_key(&session.identity()) {
                rebuilt.push((
                    registration,
                    build_session_cell(&registration.def, session, Some(event.seq.get()))?,
                ));
            }
        }
        // All cold reads must succeed before publishing any event-driven state.
        for (registration, cell) in rebuilt {
            registration.cells.lock().insert(session.identity(), cell);
        }
        for registration in registrations.values() {
            let (next, changed) = {
                let mut cells = registration.cells.lock();
                let cell = cells
                    .get_mut(&session.identity())
                    .expect("prepared projection cell");
                let next = (registration.def.apply)(&cell.state, event);
                let changed = !Arc::ptr_eq(&next, &cell.state);
                cell.state = next;
                cell.observed_seq = event.seq.get() as i64;
                (cell.state.clone(), changed)
            };
            if changed {
                let mut cells = registration.cells.lock();
                let cell = cells
                    .get_mut(&session.identity())
                    .expect("projection cell exists after drive");
                if listeners.is_empty() {
                    cell.observed_view = None;
                    continue;
                }
                let raw = (registration.def.view)(&next);
                let view_changed = cell
                    .observed_view
                    .as_ref()
                    .is_none_or(|previous| !Arc::ptr_eq(previous, &raw));
                cell.observed_view = Some(raw.clone());
                if view_changed {
                    let value = (registration.def.schema)(&raw)
                        .expect("session projection view violated its schema");
                    for (_, listener) in &listeners {
                        listener(
                            session,
                            &registration.def.key,
                            &value,
                            event.seq.get() as i64,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

fn build_session_cell(
    def: &ProjectionDefinition,
    session: &Session,
    before: Option<u64>,
) -> Result<UnitCell, String> {
    let mut cell = UnitCell {
        state: (def.init)(session.header()),
        observed_seq: -1,
        observed_view: None,
    };
    session.visit_events(0, before, |event| {
        cell.state = (def.apply)(&cell.state, event);
        cell.observed_seq = event.seq.get() as i64;
        Ok(true)
    })?;
    Ok(cell)
}

/// Restore missing units in one traversal. A detached history reader must
/// never re-populate the registry's strong Session cells.
fn cells_for(
    registrations: &HashMap<String, Registration>,
    session: &Session,
    retain: bool,
) -> Result<HashMap<String, UnitCell>, String> {
    let identity = session.identity();
    let attached = session.is_attached_to_store();
    let mut ready = HashMap::new();
    let mut missing = Vec::new();
    for registration in registrations.values() {
        let previous = if retain && attached {
            registration.cells.lock().get(&identity).cloned()
        } else {
            let removed = registration.cells.lock().remove(&identity);
            if retain { None } else { removed }
        };
        if let Some(cell) = previous {
            ready.insert(registration.def.key.clone(), cell);
        } else {
            missing.push((
                registration,
                UnitCell {
                    state: (registration.def.init)(session.header()),
                    observed_seq: -1,
                    observed_view: None,
                },
            ));
        }
    }
    if !missing.is_empty() {
        session.visit_events(0, None, |event| {
            for (registration, cell) in &mut missing {
                cell.state = (registration.def.apply)(&cell.state, event);
                cell.observed_seq = event.seq.get() as i64;
            }
            Ok(true)
        })?;
    }
    for (registration, cell) in missing {
        if retain && attached {
            registration.cells.lock().insert(identity, cell.clone());
        }
        ready.insert(registration.def.key.clone(), cell);
    }
    Ok(ready)
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
mod retirement_tests {
    use super::*;

    #[tokio::test]
    async fn archived_projection_units_match_complete_log_and_detached_checkpoint() {
        let original = Session::create(
            dsh_session::session_id("archive-projection"),
            None,
            None,
            None,
        )
        .unwrap();
        for index in 0..32 {
            original
                .append(
                    "session/title",
                    serde_json::json!({"value":index,"body":"x".repeat(8192)}),
                    None,
                )
                .unwrap();
        }
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        original
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        let restored = Session::from_event_archive(
            original.id().clone(),
            builder.finish().unwrap(),
            original.header(),
            dsh_session::SessionLogOffset::ZERO,
            vec![],
        )
        .unwrap();
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        for key in ["first", "second"] {
            registry
                .register(
                    &ctx,
                    ProjectionDefinition {
                        key: key.into(),
                        state_version: 1,
                        init: Arc::new(|_| arc(serde_json::json!({"count":0,"last":null}))),
                        apply: Arc::new(|state, event| {
                            let mut value = downcast::<serde_json::Value>(state).unwrap().clone();
                            value["count"] = (value["count"].as_u64().unwrap() + 1).into();
                            if let Some(latest) = event.data.get("value") {
                                value["last"] = latest.clone();
                            }
                            arc(value)
                        }),
                        view: Arc::new(|state| state.clone()),
                        schema: Arc::new(|state| {
                            Ok(downcast::<serde_json::Value>(state).unwrap().clone())
                        }),
                    },
                )
                .unwrap();
        }
        let complete = restored.events();
        let (expected, expected_checkpoint) = registry
            .restore(
                restored.header(),
                &ProjectionCheckpoint::new(),
                &complete,
                0,
            )
            .unwrap();
        assert_eq!(registry.try_snapshot(&restored).unwrap(), expected);
        assert_eq!(
            registry.try_checkpoint_detached(&restored).unwrap(),
            expected_checkpoint
        );
        assert_eq!(expected.values["first"], expected.values["second"]);
        assert_eq!(expected.values["first"]["last"], 31);
        assert!(
            registry
                .registrations
                .lock()
                .values()
                .all(|registration| registration.cells.lock().is_empty())
        );
        ctx.fiber.dispose().await;
    }

    #[tokio::test]
    async fn late_history_snapshot_does_not_resurrect_disposed_projection_cells() {
        let ctx = Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let registry = SessionProjectionRegistry::install(&ctx);
        registry
            .register(
                &ctx,
                ProjectionDefinition {
                    key: "late-history".into(),
                    state_version: 1,
                    init: Arc::new(|_| arc(serde_json::json!({"payload":"x".repeat(128*1024)}))),
                    apply: Arc::new(|state, _| state.clone()),
                    view: Arc::new(|state| state.clone()),
                    schema: Arc::new(|value| {
                        Ok(downcast::<serde_json::Value>(value).unwrap().clone())
                    }),
                },
            )
            .unwrap();
        for index in 0..20 {
            let session = Session::create(
                dsh_session::session_id(format!("history-race-{index}")),
                None,
                None,
                None,
            )
            .unwrap();
            let detach = store.enter(&session).unwrap();
            store.announce(&session).await.unwrap();
            registry.snapshot(&session);
            assert_eq!(
                registry.registrations.lock()["late-history"]
                    .cells
                    .lock()
                    .len(),
                1
            );
            detach().await;
            assert!(!session.is_attached_to_store());
            let value = registry.snapshot(&session);
            assert_eq!(
                value.values["late-history"]["payload"]
                    .as_str()
                    .unwrap()
                    .len(),
                128 * 1024
            );
            assert!(
                registry.registrations.lock()["late-history"]
                    .cells
                    .lock()
                    .is_empty()
            );
        }
        ctx.fiber.dispose().await;
    }

    #[tokio::test]
    async fn final_checkpoint_cannot_repopulate_retired_session_cells() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        let _registration = registry
            .register(
                &ctx,
                ProjectionDefinition {
                    key: "retirement-fixture".into(),
                    state_version: 1,
                    init: Arc::new(|_| arc(serde_json::json!({"payload":"x".repeat(16 * 1024)}))),
                    apply: Arc::new(|state, _| state.clone()),
                    view: Arc::new(|state| state.clone()),
                    schema: Arc::new(|value| {
                        Ok(downcast::<serde_json::Value>(value).unwrap().clone())
                    }),
                },
            )
            .unwrap();
        for index in 0..100 {
            let session = Session::create(
                dsh_session::session_id(format!("retired-{index}")),
                None,
                None,
                None,
            )
            .unwrap();
            registry.snapshot(&session);
            registry.forget_session(&session);
            let checkpoint = registry.checkpoint_detached(&session);
            assert_eq!(
                checkpoint["retirement-fixture"].val["payload"]
                    .as_str()
                    .unwrap()
                    .len(),
                16 * 1024
            );
            assert!(
                registry.registrations.lock()["retirement-fixture"]
                    .cells
                    .lock()
                    .is_empty()
            );
        }
    }
}
