#![allow(clippy::type_complexity)]
// Generic reservation tables and commit callbacks intentionally retain their exact ownership types.

//! Bounded sharing and exclusive reservation of unpublished Sessions.
//! Rust port of `packages/session/session-persistence/src/preparations.ts`.
//!
//! # Deviations
//!
//! - `AbortSignal` observers are omitted (no cancellation wiring yet); the
//!   queued-read sharing contract (`inspect`/`reserve` share one in-flight
//!   load per id) is preserved.
//! - The shared in-flight result uses `tokio::sync::OnceCell` + `Notify`.

use std::sync::Arc;

use dsh_session::{Session, SessionId};
use indexmap::IndexMap;
use parking_lot::Mutex;
use tokio::sync::{Notify, OnceCell};

/// One prepared source exposing its exact unpublished Session.
pub trait PreparedSource: Send + Sync + 'static {
    fn session(&self) -> &Session;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PreparationPhase {
    Loading,
    Ready,
    Committing,
    Reserved,
}

struct EntryState<S: PreparedSource, C> {
    phase: PreparationPhase,
    source: Option<Arc<S>>,
    reservation: Option<Arc<SessionPreparationReservation<S, C>>>,
}

/// One preparation entry: shared in-flight load plus reservation lifecycle.
pub struct PreparationEntry<S: PreparedSource, C> {
    id: SessionId,
    result: OnceCell<Result<Arc<S>, String>>,
    notify: Notify,
    state: Mutex<EntryState<S, C>>,
}

/// One exclusively held prepared source and its committed persistence state.
pub struct SessionPreparationReservation<S: PreparedSource, C> {
    pub entry: Arc<PreparationEntry<S, C>>,
    pub source: Arc<S>,
    pub state: C,
}

/// A cold-source loader shared by inspect/reserve.
pub type PreparedSourceLoader<S> =
    Arc<dyn Fn() -> crate::coordinator::BoxOpFuture<Arc<S>> + Send + Sync>;

type LoadFn<S> = PreparedSourceLoader<S>;

/// Per-coordinator cold-read sharing, exclusive reservation, and ready-entry
/// LRU (TS `SessionPreparations`).
pub struct SessionPreparations<S: PreparedSource, C> {
    capacity: usize,
    entries: Arc<Mutex<IndexMap<String, Arc<PreparationEntry<S, C>>>>>,
}

impl<S: PreparedSource, C: Clone + Send + Sync + 'static> SessionPreparations<S, C> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Arc::new(Mutex::new(IndexMap::new())),
        }
    }

    /// Whether this pool currently knows about an unpublished identity.
    pub fn has(&self, id: &SessionId) -> bool {
        self.entries.lock().contains_key(id.as_str())
    }

    /// Observe one prepared source, sharing an in-flight read for the same id.
    pub async fn inspect(&self, id: &SessionId, load: LoadFn<S>) -> Result<Arc<S>, String> {
        let entry = self.entry_for(id, load);
        let loaded = self.await_result(&entry).await?;
        let source = {
            let state = entry.state.lock();
            state.source.clone().unwrap_or(loaded.clone())
        };
        if self.is_current(&entry, id) && entry.state.lock().phase == PreparationPhase::Ready {
            self.touch(&entry);
        }
        Ok(source)
    }

    /// Reserve one ready source after committing its pending durable repair.
    pub async fn reserve(
        &self,
        id: &SessionId,
        load: LoadFn<S>,
        commit: Arc<
            dyn Fn(Arc<S>) -> crate::coordinator::BoxOpFuture<Option<(Arc<S>, C)>> + Send + Sync,
        >,
    ) -> Result<Option<Arc<SessionPreparationReservation<S, C>>>, String> {
        let entry = self.entry_for(id, load);
        let _ = self.await_result(&entry).await?;
        loop {
            let phase = entry.state.lock().phase;
            if phase == PreparationPhase::Ready {
                break;
            }
            if !self.is_current(&entry, id) {
                return Ok(None);
            }
            let notified = entry.notify.notified();
            notified.await;
        }
        if !self.is_current(&entry, id) {
            return Ok(None);
        }
        let source = entry
            .state
            .lock()
            .source
            .clone()
            .expect("ready entry carries its source");
        entry.state.lock().phase = PreparationPhase::Committing;
        let committed = match commit(source).await {
            Ok(committed) => committed,
            Err(error) => {
                self.remove(&entry);
                return Err(error);
            }
        };
        let Some(committed) = committed else {
            self.remove(&entry);
            return Ok(None);
        };
        entry.state.lock().source = Some(committed.0.clone());
        if !self.is_current(&entry, id) {
            return Ok(None);
        }
        let reservation = Arc::new(SessionPreparationReservation {
            entry: entry.clone(),
            source: committed.0,
            state: committed.1,
        });
        {
            let mut state = entry.state.lock();
            state.phase = PreparationPhase::Reserved;
            state.reservation = Some(reservation.clone());
        }
        Ok(Some(reservation))
    }

    /// Return the exact reservation for Session publication, rejecting
    /// aliases (TS `reservationFor`).
    pub fn reservation_for(
        &self,
        session: &Session,
    ) -> Result<Option<Arc<SessionPreparationReservation<S, C>>>, String> {
        let entry = self.entries.lock().get(session.id().as_str()).cloned();
        let Some(entry) = entry else {
            return Ok(None);
        };
        let state = entry.state.lock();
        if state.phase == PreparationPhase::Reserved
            && state
                .source
                .as_ref()
                .is_some_and(|source| source.session().ptr_eq(session))
            && state.reservation.is_some()
        {
            return Ok(state.reservation.clone());
        }
        Err(format!(
            "cannot publish session \"{}\": persisted state already owns this identity",
            session.id().as_str()
        ))
    }

    /// Consume a reservation after its exact Session has attached.
    pub fn attach(
        &self,
        reservation: &Arc<SessionPreparationReservation<S, C>>,
    ) -> Result<(), String> {
        let entry = &reservation.entry;
        let current = self
            .entries
            .lock()
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current
            || entry.state.lock().reservation.as_ref().map(Arc::as_ptr)
                != Some(Arc::as_ptr(reservation))
        {
            return Err(format!(
                "session \"{}\" preparation is no longer reserved",
                entry.id.as_str()
            ));
        }
        self.remove(entry);
        Ok(())
    }

    /// Consume a reservation whose caller only needs the committed
    /// inspection.
    pub fn discard(&self, reservation: &Arc<SessionPreparationReservation<S, C>>) {
        let entry = &reservation.entry;
        let current = self
            .entries
            .lock()
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current {
            return;
        }
        if entry.state.lock().reservation.as_ref().map(Arc::as_ptr)
            == Some(Arc::as_ptr(reservation))
        {
            self.remove(entry);
        }
    }

    /// Return a reusable unpublished reservation to the ready LRU.
    pub fn release(&self, reservation: &Arc<SessionPreparationReservation<S, C>>, reusable: bool) {
        let entry = &reservation.entry;
        let current = self
            .entries
            .lock()
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current
            || entry.state.lock().reservation.as_ref().map(Arc::as_ptr)
                != Some(Arc::as_ptr(reservation))
            || entry.state.lock().phase != PreparationPhase::Reserved
        {
            return;
        }
        if !reusable {
            self.remove(entry);
            return;
        }
        entry.state.lock().reservation = None;
        self.make_ready(entry);
    }

    /// Discard a prepared view after the durable log changes.
    pub fn invalidate(&self, id: &SessionId) {
        let entry = { self.entries.lock().get(id.as_str()).cloned() };
        if let Some(entry) = entry {
            self.remove(&entry);
        }
    }

    /// Discard an exact stale ready source without disturbing an exclusive
    /// owner.
    pub fn discard_ready(&self, id: &SessionId, expected: &Arc<S>) -> DiscardOutcome {
        let entry = self.entries.lock().get(id.as_str()).cloned();
        let Some(entry) = entry else {
            return DiscardOutcome::Missing;
        };
        let source_is_expected = entry
            .state
            .lock()
            .source
            .as_ref()
            .is_some_and(|source| Arc::ptr_eq(source, expected));
        if !source_is_expected {
            return DiscardOutcome::Missing;
        }
        if entry.state.lock().phase != PreparationPhase::Ready {
            return DiscardOutcome::Retained;
        }
        self.remove(&entry);
        DiscardOutcome::Discarded
    }

    /// Reject writes while an unpublished Session exclusively reserves the
    /// id.
    pub fn assert_writable(&self, id: &SessionId) -> Result<(), String> {
        let phase = self
            .entries
            .lock()
            .get(id.as_str())
            .map(|entry| entry.state.lock().phase);
        if matches!(
            phase,
            Some(PreparationPhase::Committing | PreparationPhase::Reserved)
        ) {
            return Err(format!(
                "cannot append session \"{}\" while its persisted preparation is reserved",
                id.as_str()
            ));
        }
        Ok(())
    }

    /// Remove a completed entry for an already-serialized append adoption.
    pub fn take_ready(&self, id: &SessionId) -> Option<Arc<S>> {
        let entry = self.entries.lock().get(id.as_str()).cloned()?;
        if entry.state.lock().phase != PreparationPhase::Ready {
            return None;
        }
        let source = entry.state.lock().source.clone()?;
        self.remove(&entry);
        Some(source)
    }

    fn entry_for(&self, id: &SessionId, load: LoadFn<S>) -> Arc<PreparationEntry<S, C>> {
        let entry = {
            let mut entries = self.entries.lock();
            if let Some(existing) = entries.get(id.as_str()).cloned() {
                return existing;
            }
            let entry = Arc::new(PreparationEntry {
                id: id.clone(),
                result: OnceCell::new(),
                notify: Notify::new(),
                state: Mutex::new(EntryState {
                    phase: PreparationPhase::Loading,
                    source: None,
                    reservation: None,
                }),
            });
            entries.insert(id.as_str().to_string(), entry.clone());
            entry
        };
        // Start immediately (the TS `entryFor` starts the load synchronously),
        // settling the shared result after the entry becomes ready.
        let entries = Arc::clone(&self.entries);
        let entry_for_task = entry.clone();
        let id_for_task = id.clone();
        let capacity = self.capacity;
        tokio::spawn(async move {
            let result = load().await;
            let current = entries
                .lock()
                .get(id_for_task.as_str())
                .is_some_and(|live| Arc::ptr_eq(live, &entry_for_task));
            if current {
                match &result {
                    Ok(source) => {
                        entry_for_task.state.lock().source = Some(source.clone());
                        entry_for_task.state.lock().phase = PreparationPhase::Ready;
                        entry_for_task.notify.notify_waiters();
                        touch_entry(&entries, &entry_for_task, capacity);
                    }
                    Err(_) => {
                        entries.lock().shift_remove(id_for_task.as_str());
                    }
                }
            }
            let _ = entry_for_task.result.set(result);
            entry_for_task.notify.notify_waiters();
        });
        entry
    }

    async fn await_result(&self, entry: &Arc<PreparationEntry<S, C>>) -> Result<Arc<S>, String> {
        loop {
            if let Some(result) = entry.result.get() {
                return result.clone();
            }
            let notified = entry.notify.notified();
            notified.await;
        }
    }

    fn is_current(&self, entry: &Arc<PreparationEntry<S, C>>, id: &SessionId) -> bool {
        self.entries
            .lock()
            .get(id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry))
    }

    fn make_ready(&self, entry: &Arc<PreparationEntry<S, C>>) {
        if !self.is_current(entry, &entry.id) {
            return;
        }
        entry.state.lock().phase = PreparationPhase::Ready;
        entry.notify.notify_waiters();
        self.touch(entry);
    }

    fn remove(&self, entry: &Arc<PreparationEntry<S, C>>) {
        let current = self
            .entries
            .lock()
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current {
            return;
        }
        self.entries.lock().shift_remove(entry.id.as_str());
        entry.notify.notify_waiters();
    }

    fn touch(&self, entry: &Arc<PreparationEntry<S, C>>) {
        touch_entry(&self.entries, entry, self.capacity);
    }
}

/// Move one entry to the LRU tail and evict the least-recently-used ready
/// entry beyond capacity.
fn touch_entry<S: PreparedSource, C>(
    entries: &Arc<Mutex<IndexMap<String, Arc<PreparationEntry<S, C>>>>>,
    entry: &Arc<PreparationEntry<S, C>>,
    capacity: usize,
) {
    let mut entries_guard = entries.lock();
    entries_guard.shift_remove(entry.id.as_str());
    entries_guard.insert(entry.id.as_str().to_string(), entry.clone());
    let ready_count = entries_guard
        .values()
        .filter(|candidate| candidate.state.lock().phase == PreparationPhase::Ready)
        .count();
    if ready_count <= capacity {
        return;
    }
    if let Some(index) = entries_guard
        .values()
        .position(|candidate| candidate.state.lock().phase == PreparationPhase::Ready)
    {
        entries_guard.shift_remove_index(index);
    }
}

/// The `discardReady` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardOutcome {
    Discarded,
    Retained,
    Missing,
}
