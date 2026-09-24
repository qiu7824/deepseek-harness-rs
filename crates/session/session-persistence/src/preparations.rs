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
use std::sync::atomic::{AtomicUsize, Ordering};

use dsh_session::{Session, SessionId};
use indexmap::IndexMap;
use parking_lot::Mutex;
use tokio::sync::{Notify, OnceCell};

const MAX_CACHED_PREPARED_EVENTS: u64 = 4096;
const MAX_CACHED_PREPARED_BYTES: usize = 8 * 1024 * 1024;

/// Estimate retained JSON storage without serializing or cloning the log.
/// Stop at the budget: a single image or tool result can outweigh thousands
/// of ordinary events, so an event-count limit alone is insufficient.
fn fits_prepared_cache(session: &Session) -> bool {
    fn charge(remaining: &mut usize, bytes: usize) -> bool {
        match remaining.checked_sub(bytes) {
            Some(next) => {
                *remaining = next;
                true
            }
            None => false,
        }
    }
    fn json_fits(value: &serde_json::Value, remaining: &mut usize) -> bool {
        use serde_json::Value;
        match value {
            Value::String(text) => charge(remaining, text.capacity()),
            Value::Array(items) => {
                charge(
                    remaining,
                    items
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Value>()),
                ) && items.iter().all(|item| json_fits(item, remaining))
            }
            Value::Object(items) => {
                charge(
                    remaining,
                    items
                        .len()
                        .saturating_mul(std::mem::size_of::<(String, Value)>()),
                ) && items.iter().all(|(key, value)| {
                    charge(remaining, key.capacity()) && json_fits(value, remaining)
                })
            }
            _ => true,
        }
    }
    session.with_events(|events| {
        if events.len() as u64 > MAX_CACHED_PREPARED_EVENTS {
            return false;
        }
        let mut remaining = MAX_CACHED_PREPARED_BYTES;
        charge(
            &mut remaining,
            events
                .len()
                .saturating_mul(std::mem::size_of::<dsh_session::SessionEvent>()),
        ) && events.iter().all(|event| {
            charge(&mut remaining, event.type_.capacity())
                && charge(
                    &mut remaining,
                    event.source_event_seqs.as_ref().map_or(0, |seqs| {
                        seqs.capacity().saturating_mul(std::mem::size_of::<u64>())
                    }),
                )
                && json_fits(&event.data, &mut remaining)
        })
    })
}

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
    cacheable: bool,
}

/// One preparation entry: shared in-flight load plus reservation lifecycle.
pub struct PreparationEntry<S: PreparedSource, C> {
    id: SessionId,
    result: OnceCell<Result<Arc<S>, String>>,
    notify: Notify,
    state: Mutex<EntryState<S, C>>,
    readers: AtomicUsize,
}

/// A cancelled inspect/reserve must release its cache admission as well.
struct PreparationRead<S: PreparedSource, C> {
    entries: Arc<Mutex<IndexMap<String, Arc<PreparationEntry<S, C>>>>>,
    entry: Arc<PreparationEntry<S, C>>,
}

impl<S: PreparedSource, C> Drop for PreparationRead<S, C> {
    fn drop(&mut self) {
        if self.entry.readers.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        discard_unobserved_large_entry(&self.entries, &self.entry);
    }
}

fn discard_unobserved_large_entry<S: PreparedSource, C>(
    entries: &Arc<Mutex<IndexMap<String, Arc<PreparationEntry<S, C>>>>>,
    entry: &Arc<PreparationEntry<S, C>>,
) {
    let mut entries = entries.lock();
    if entry.readers.load(Ordering::Acquire) != 0 {
        return;
    }
    let state = entry.state.lock();
    if state.phase == PreparationPhase::Ready
        && !state.cacheable
        && entries
            .get(entry.id.as_str())
            .is_some_and(|current| Arc::ptr_eq(current, entry))
    {
        entries.shift_remove(entry.id.as_str());
        entry.notify.notify_waiters();
    }
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
        let (entry, _read) = self.entry_for(id, load);
        let loaded = self.await_result(&entry).await?;
        let source = {
            let state = entry.state.lock();
            state.source.clone().unwrap_or(loaded.clone())
        };
        if self.is_current(&entry, id) && entry.state.lock().phase == PreparationPhase::Ready {
            if !fits_prepared_cache(source.session()) {
                self.discard_ready(id, &source);
            } else {
                self.touch(&entry);
            }
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
        let (entry, _read) = self.entry_for(id, load);
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

    /// Release all cache entries and reverse reservations at backend teardown.
    pub fn clear(&self) {
        let entries: Vec<_> = self.entries.lock().drain(..).map(|(_, entry)| entry).collect();
        for entry in entries {
            entry.state.lock().reservation = None;
            entry.notify.notify_waiters();
        }
    }

    /// Discard an exact stale ready source without disturbing an exclusive
    /// owner.
    pub fn discard_ready(&self, id: &SessionId, expected: &Arc<S>) -> DiscardOutcome {
        let mut entries = self.entries.lock();
        let entry = entries.get(id.as_str()).cloned();
        let Some(entry) = entry else {
            return DiscardOutcome::Missing;
        };
        let state = entry.state.lock();
        let source_is_expected = state
            .source
            .as_ref()
            .is_some_and(|source| Arc::ptr_eq(source, expected));
        if !source_is_expected {
            return DiscardOutcome::Missing;
        }
        if state.phase != PreparationPhase::Ready {
            return DiscardOutcome::Retained;
        }
        entries.shift_remove(id.as_str());
        entry.notify.notify_waiters();
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

    fn entry_for(
        &self,
        id: &SessionId,
        load: LoadFn<S>,
    ) -> (Arc<PreparationEntry<S, C>>, PreparationRead<S, C>) {
        let entry = {
            let mut entries = self.entries.lock();
            if let Some(existing) = entries.get(id.as_str()).cloned() {
                existing.readers.fetch_add(1, Ordering::Relaxed);
                let read = PreparationRead {
                    entries: self.entries.clone(),
                    entry: existing.clone(),
                };
                return (existing, read);
            }
            let entry = Arc::new(PreparationEntry {
                id: id.clone(),
                result: OnceCell::new(),
                notify: Notify::new(),
                state: Mutex::new(EntryState {
                    phase: PreparationPhase::Loading,
                    source: None,
                    reservation: None,
                    cacheable: true,
                }),
                readers: AtomicUsize::new(1),
            });
            entries.insert(id.as_str().to_string(), entry.clone());
            entry
        };
        let read = PreparationRead {
            entries: self.entries.clone(),
            entry: entry.clone(),
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
                        let cacheable = fits_prepared_cache(source.session());
                        {
                            let mut state = entry_for_task.state.lock();
                            state.source = Some(source.clone());
                            state.cacheable = cacheable;
                            state.phase = PreparationPhase::Ready;
                        }
                        entry_for_task.notify.notify_waiters();
                        touch_entry(&entries, &entry_for_task, capacity);
                    }
                    Err(_) => {
                        entries.lock().shift_remove(id_for_task.as_str());
                    }
                }
            }
            let _ = entry_for_task.result.set(result);
            discard_unobserved_large_entry(&entries, &entry_for_task);
            entry_for_task.notify.notify_waiters();
        });
        (entry, read)
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
        let source = entry.state.lock().source.clone();
        if let Some(source) = source
            && !fits_prepared_cache(source.session())
        {
            self.discard_ready(&entry.id, &source);
            return;
        }
        self.touch(entry);
    }

    fn remove(&self, entry: &Arc<PreparationEntry<S, C>>) {
        let mut entries = self.entries.lock();
        let current = entries
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current {
            return;
        }
        entries.shift_remove(entry.id.as_str());
        // A reservation owns its entry. Release the reverse owner when the
        // entry leaves the pool, including successful attachment; otherwise
        // every restored session leaks its prepared log and inspection.
        entry.state.lock().reservation = None;
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
    // A late loader cannot resurrect an invalidated generation.
    if !entries_guard
        .get(entry.id.as_str())
        .is_some_and(|current| Arc::ptr_eq(current, entry))
    {
        return;
    }
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

#[cfg(test)]
mod retention_tests {
    use super::*;
    struct Source(Session);
    impl PreparedSource for Source {
        fn session(&self) -> &Session {
            &self.0
        }
    }
    fn source(count: usize) -> Arc<Source> {
        let session = Session::create(
            dsh_session::session_id("prepared-retention"),
            None,
            None,
            None,
        )
        .unwrap();
        for _ in 0..count {
            session
                .append("retention-fixture", serde_json::json!({}), None)
                .unwrap();
        }
        Arc::new(Source(session))
    }
    fn loader(source: &Arc<Source>) -> LoadFn<Source> {
        let source = source.clone();
        Arc::new(move || {
            let source = source.clone();
            Box::pin(async move { Ok(source) })
        })
    }
    #[tokio::test]
    async fn consumed_reservations_release_the_prepared_source() {
        for operation in ["attach", "discard", "release"] {
            let source = source(10);
            let weak_source = Arc::downgrade(&source);
            let pool = SessionPreparations::<Source, ()>::new(1);
            let reservation = pool
                .reserve(
                    source.session().id(),
                    loader(&source),
                    Arc::new(|source| Box::pin(async move { Ok(Some((source, ()))) })),
                )
                .await
                .unwrap()
                .unwrap();
            let weak_entry = Arc::downgrade(&reservation.entry);
            match operation {
                "attach" => pool.attach(&reservation).unwrap(),
                "discard" => pool.discard(&reservation),
                _ => pool.release(&reservation, false),
            }
            drop(reservation);
            drop(source);
            assert!(
                weak_entry.upgrade().is_none(),
                "{operation} retained its reservation"
            );
            assert!(
                weak_source.upgrade().is_none(),
                "{operation} retained its prepared history"
            );
        }
    }
    #[tokio::test]
    async fn large_read_only_preparations_are_not_kept_after_inspection() {
        let source = source(4100);
        let pool = SessionPreparations::<Source, ()>::new(5);
        let found = pool
            .inspect(source.session().id(), loader(&source))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&source, &found));
        assert!(!pool.has(source.session().id()));
    }
    #[tokio::test]
    async fn reservations_survive_inspection_and_large_release_drops_the_cache() {
        let source = source(4100);
        let pool = SessionPreparations::<Source, ()>::new(5);
        let reservation = pool
            .reserve(
                source.session().id(),
                loader(&source),
                Arc::new(|source| Box::pin(async move { Ok(Some((source, ()))) })),
            )
            .await
            .unwrap()
            .unwrap();
        pool.inspect(source.session().id(), loader(&source))
            .await
            .unwrap();
        assert!(pool.has(source.session().id()));
        pool.release(&reservation, true);
        assert!(!pool.has(source.session().id()));
    }
    #[tokio::test]
    async fn small_preparations_keep_the_existing_reuse_behavior() {
        let source = source(10);
        let pool = SessionPreparations::<Source, ()>::new(1);
        pool.inspect(source.session().id(), loader(&source))
            .await
            .unwrap();
        assert!(pool.has(source.session().id()));
    }

    #[tokio::test]
    async fn a_single_large_payload_is_not_retained_after_inspect_or_release() {
        for reserve in [false, true] {
            let source = source(1);
            source
                .session()
                .append(
                    "large-tool-result",
                    serde_json::json!({
                        "output": "x".repeat(MAX_CACHED_PREPARED_BYTES)
                    }),
                    None,
                )
                .unwrap();
            let weak = Arc::downgrade(&source);
            let pool = SessionPreparations::<Source, ()>::new(5);
            if reserve {
                let reservation = pool
                    .reserve(
                        source.session().id(),
                        loader(&source),
                        Arc::new(|source| Box::pin(async move { Ok(Some((source, ()))) })),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let inspected = pool
                    .inspect(source.session().id(), loader(&source))
                    .await
                    .unwrap();
                assert!(
                    pool.has(source.session().id()),
                    "inspection must preserve a reserved source"
                );
                drop(inspected);
                pool.release(&reservation, true);
                drop(reservation);
            } else {
                drop(
                    pool.inspect(source.session().id(), loader(&source))
                        .await
                        .unwrap(),
                );
            }
            assert!(!pool.has(source.session().id()));
            drop(source);
            assert!(
                weak.upgrade().is_none(),
                "large payload remains owned by the cache"
            );
        }
    }

    #[tokio::test]
    async fn cancelling_inspection_during_load_does_not_cache_a_large_source() {
        let source = source(1);
        source
            .session()
            .append(
                "large-result",
                serde_json::json!({
                    "output": "x".repeat(MAX_CACHED_PREPARED_BYTES)
                }),
                None,
            )
            .unwrap();
        let id = source.session().id().clone();
        let weak = Arc::downgrade(&source);
        let pool = Arc::new(SessionPreparations::<Source, ()>::new(5));
        let (started, start) = tokio::sync::oneshot::channel();
        let (finish, done) = tokio::sync::oneshot::channel();
        let controls = Arc::new(Mutex::new(Some((started, done))));
        let load: LoadFn<Source> = Arc::new(move || {
            let source = source.clone();
            let (started, done) = controls.lock().take().unwrap();
            Box::pin(async move {
                started.send(()).unwrap();
                done.await.unwrap();
                Ok(source)
            })
        });
        let task = tokio::spawn({
            let pool = pool.clone();
            let id = id.clone();
            async move { pool.inspect(&id, load).await }
        });
        start.await.unwrap();
        task.abort();
        assert!(task.await.is_err());
        finish.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled read retained a large prepared source");
        assert!(!pool.has(&id));
    }
}
