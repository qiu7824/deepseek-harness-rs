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
    pub fn attach(&self, reservation: &Arc<SessionPreparationReservation<S, C>>) -> Result<(), String> {
        let entry = &reservation.entry;
        let current = self
            .entries
            .lock()
            .get(entry.id.as_str())
            .is_some_and(|live| Arc::ptr_eq(live, entry));
        if !current || entry.state.lock().reservation.as_ref().map(Arc::as_ptr)
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
        if let Some(entry) = self.entries.lock().get(id.as_str()).cloned() {
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
        if matches!(phase, Some(PreparationPhase::Committing | PreparationPhase::Reserved)) {
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
        if let Some(existing) = self.entries.lock().get(id.as_str()).cloned() {
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
        self.entries.lock().insert(id.as_str().to_string(), entry.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SESSION_FORMAT_VERSION, SessionHeader, session_id};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Clone)]
    struct TestSource {
        session: Arc<Session>,
        label: &'static str,
    }

    impl PreparedSource for TestSource {
        fn session(&self) -> &Session {
            &self.session
        }
    }

    fn make_source(id: &str, label: &'static str) -> Arc<TestSource> {
        let session = Session::create(session_id(id), None, None).unwrap();
        Arc::new(TestSource { session: Arc::new(session), label })
    }

    fn header(id: &str) -> SessionHeader {
        SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: session_id(id),
            created_at: 0,
            cwd: None,
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inspect_shares_one_in_flight_load() {
        let loads = Arc::new(AtomicU32::new(0));
        let pool: SessionPreparations<TestSource, ()> = SessionPreparations::new(2);
        let load = {
            let loads = loads.clone();
            Arc::new(move || {
                let loads = loads.clone();
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(make_source("s1", "first"))
                }) as crate::coordinator::BoxOpFuture<Arc<TestSource>>
            })
        };
        let id = session_id("s1");
        let (first, second) = tokio::join!(
            pool.inspect(&id, load.clone()),
            pool.inspect(&id, load.clone())
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one shared cold read");
        assert_eq!(first.unwrap().label, "first");
        assert_eq!(second.unwrap().label, "first");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_load_is_retried_by_a_later_inspect() {
        let loads = Arc::new(AtomicU32::new(0));
        let pool: SessionPreparations<TestSource, ()> = SessionPreparations::new(2);
        let load = {
            let loads = loads.clone();
            Arc::new(move || {
                let loads = loads.clone();
                Box::pin(async move {
                    if loads.fetch_add(1, Ordering::SeqCst) == 0 {
                        return Err("backend read failed".to_string());
                    }
                    Ok(make_source("s1", "second"))
                }) as crate::coordinator::BoxOpFuture<Arc<TestSource>>
            })
        };
        let id = session_id("s1");
        let error = pool
            .inspect(&id, load.clone())
            .await
            .err()
            .expect("read fails");
        assert!(error.contains("backend read failed"), "{error}");
        // The failed entry was removed; the next call retries.
        let source = pool
            .inspect(&id, load.clone())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(source.label, "second");
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reserve_commit_attach_lifecycle() {
        let pool: SessionPreparations<TestSource, ()> = SessionPreparations::new(2);
        let load: LoadFn<TestSource> = Arc::new(move || {
            Box::pin(async move { Ok(make_source("s1", "cold")) })
        });
        let commit: Arc<
            dyn Fn(Arc<TestSource>) -> crate::coordinator::BoxOpFuture<Option<(Arc<TestSource>, ())>>
                + Send
                + Sync,
        > = Arc::new(move |source| {
            Box::pin(async move { Ok(Some((source, ()))) })
        });

        let reservation = pool
            .reserve(&session_id("s1"), load, commit)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .expect("reservation");

        // Reserved entries reject writes and reject alias publication.
        assert!(pool.assert_writable(&session_id("s1")).is_err());
        let other = Session::create(session_id("s1"), None, None).unwrap();
        assert!(pool.reservation_for(&other).is_err());

        // The exact session resolves.
        let exact = reservation.source.session().clone();
        let resolved = pool.reservation_for(&exact).unwrap().expect("reservation");
        pool.attach(&resolved).unwrap();
        assert!(!pool.has(&session_id("s1")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_reusable_returns_to_ready_lru() {
        let pool: SessionPreparations<TestSource, ()> = SessionPreparations::new(2);
        let load: LoadFn<TestSource> = Arc::new(move || {
            Box::pin(async move { Ok(make_source("s1", "cold")) })
        });
        let commit: Arc<
            dyn Fn(Arc<TestSource>) -> crate::coordinator::BoxOpFuture<Option<(Arc<TestSource>, ())>>
                + Send
                + Sync,
        > = Arc::new(move |source| Box::pin(async move { Ok(Some((source, ()))) }));

        let reservation = pool
            .reserve(&session_id("s1"), load.clone(), commit.clone())
            .await
            .unwrap()
            .expect("reservation");
        pool.release(&reservation, true);
        assert!(pool.has(&session_id("s1")));
        // takeReady returns the reusable source.
        let source = pool.take_ready(&session_id("s1")).expect("ready source");
        assert_eq!(source.label, "cold");
        assert!(!pool.has(&session_id("s1")));

        // Non-reusable release discards.
        let reservation = pool
            .reserve(&session_id("s1"), load.clone(), commit)
            .await
            .unwrap()
            .expect("reservation");
        pool.release(&reservation, false);
        assert!(!pool.has(&session_id("s1")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lru_evicts_oldest_ready_entry() {
        let pool: SessionPreparations<TestSource, ()> = SessionPreparations::new(1);
        let load = |id: &'static str| -> LoadFn<TestSource> {
            Arc::new(move || {
                Box::pin(async move { Ok(make_source(id, id)) })
            })
        };
        let id_a = session_id("a");
        let id_b = session_id("b");
        pool.inspect(&id_a, load("a")).await.unwrap();
        pool.inspect(&id_b, load("b")).await.unwrap();
        // Capacity 1: touching b evicted a.
        assert!(!pool.has(&id_a));
        assert!(pool.has(&id_b));
        // Touching b again keeps it.
        pool.inspect(&id_b, load("b")).await.unwrap();
        assert!(pool.has(&id_b));
    }

    #[test]
    fn header_helper_used_in_signatures() {
        let _ = header("s1");
    }
}
