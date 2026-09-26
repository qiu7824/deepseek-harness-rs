//! Shared Host management, admission, and cancellation-safe durable mutations.
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use cordis::{Context, Service, arc};
use dsh_llm::UserMessage;
use dsh_storage_domain::DomainFacility;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, Notify, OnceCell, OwnedMutexGuard, watch};

use crate::calendar::{create_record, resolve_update};
use crate::host_history::history_page;
use crate::host_store::HostStore;
use crate::host_types::*;

pub type Cancelled = Arc<dyn Fn() -> bool + Send + Sync>;
type Result<T> = std::result::Result<T, ScheduleError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub delivery_history_days: u32,
    pub delivery_history_records: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            delivery_history_days: 30,
            delivery_history_records: 200,
        }
    }
}
impl Config {
    pub fn retention(self) -> RetentionBounds {
        RetentionBounds {
            days: self.delivery_history_days,
            records: self.delivery_history_records,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.retention().validate()
    }
}

#[async_trait::async_trait]
pub trait ScheduleSessionController: Send + Sync {
    /// Own the same per-session admission used by archive and permanent deletion.
    /// Cold management validates the header without constructing an Agent.
    async fn acquire(&self, session_id: &str, wake: bool) -> Result<Box<dyn ScheduleSessionLease>>;
}

#[async_trait::async_trait]
pub trait ScheduleSessionLease: Send + Sync {
    /// Return only after the inbox append has received a real persistence acknowledgment.
    async fn deliver(&self, message: UserMessage) -> Result<()>;
}

pub struct ScheduleService {
    pub(crate) ctx: Context,
    pub(crate) controller: Arc<dyn ScheduleSessionController>,
    pub(crate) gate: Arc<AsyncMutex<()>>,
    lifecycle: AsyncMutex<()>,
    store: OnceCell<Arc<HostStore>>,
    pub(crate) config: RwLock<Config>,
    enabled: AtomicBool,
    stop_requested: AtomicBool,
    closed: AtomicBool,
    pub(crate) wake: Notify,
    pub(crate) request_revision: AtomicU64,
    pub(crate) generation: watch::Sender<u64>,
    driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Service for ScheduleService {
    fn service_name(&self) -> &'static str {
        "schedule"
    }
}

impl ScheduleService {
    pub fn install(ctx: &Context, controller: Arc<dyn ScheduleSessionController>) -> Arc<Self> {
        Self::install_with_clock(
            ctx,
            controller,
            Arc::new(|| chrono::Utc::now().timestamp_millis()),
        )
    }

    pub fn install_with_clock(
        ctx: &Context,
        controller: Arc<dyn ScheduleSessionController>,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Arc<Self> {
        let (generation, _) = watch::channel(0);
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            controller,
            clock,
            gate: Arc::new(AsyncMutex::new(())),
            lifecycle: AsyncMutex::new(()),
            store: OnceCell::new(),
            config: RwLock::new(Config::default()),
            enabled: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            wake: Notify::new(),
            generation,
            request_revision: AtomicU64::new(0),
            driver: Mutex::new(None),
        });
        ctx.register_service(service.clone());
        let weak = Arc::downgrade(&service);
        let _ = ctx.effect(
            "schedule.host",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.shutdown().await;
                        }
                    })
                }))
            }),
        );
        service
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    pub fn request_drive(&self) {
        self.request_revision.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
    }
    pub fn config(&self) -> Config {
        *self.config.read()
    }

    /// Compact operational state without reminder content or session history.
    #[doc(hidden)]
    pub fn diagnostics(&self) -> Value {
        let now = (self.clock)();
        let store = self.store.get();
        let empty = std::collections::HashSet::new();
        json!({
            "enabled": self.enabled(),
            "closed": self.closed.load(Ordering::SeqCst),
            "stopRequested": self.stop_requested.load(Ordering::SeqCst),
            "driverFinished": self.driver.lock().as_ref().map(tokio::task::JoinHandle::is_finished),
            "generation": *self.generation.borrow(),
            "requestRevision": self.request_revision.load(Ordering::SeqCst),
            "storeLoaded": store.is_some(),
            "gateBusy": self.gate.try_lock().is_err(),
            "now": now,
            "nextTarget": store.and_then(|store| store.next_target(&empty)),
            "dueCount": store.map(|store| store.due(now, &empty).len()),
        })
    }
    /// Keep a disabled plugin's validated Profile configuration available to
    /// cold management without opening storage or starting the driver.
    pub fn configure(&self, config: Config) -> Result<()> {
        config.validate()?;
        let changed = {
            let mut current = self.config.write();
            if *current == config {
                false
            } else {
                *current = config;
                true
            }
        };
        if changed {
            self.changed();
        }
        Ok(())
    }

    pub(crate) fn available(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ScheduleError::new(
                "schedule_unavailable",
                "The Host reminder service is closed.",
            ));
        }
        Ok(())
    }
    fn writable(&self) -> Result<()> {
        self.available()?;
        if !self.enabled() {
            return Err(ScheduleError::new(
                "schedule_disabled",
                "Enable scheduled tasks before creating or editing a reminder.",
            ));
        }
        Ok(())
    }

    pub(crate) async fn store(self: &Arc<Self>) -> Result<Arc<HostStore>> {
        self.available()?;
        if let Some(store) = self.store.get() {
            return Ok(store.clone());
        }
        // The initialization itself owns its future. Cancelling its first caller
        // cannot strand a domain reservation or lose the initialized store.
        let service = self.clone();
        accepted(async move {
            let _guard = service.gate.clone().lock_owned().await;
            service.available()?;
            if let Some(store) = service.store.get() {
                return Ok(store.clone());
            }
            let facility = service
                .ctx
                .get_typed::<Arc<DomainFacility>>("storageDomain", false)
                .map(|slot| slot.as_ref().clone())
                .ok_or_else(|| {
                    ScheduleError::new("schedule_unavailable", "Host storage is not available.")
                })?;
            let store = HostStore::open(&facility).await?;
            if let Err(error) = service.available() {
                store.close().await;
                return Err(error);
            }
            // Publish while holding the shutdown gate, not after an initializer
            // future has returned and released that gate.
            if service.store.set(store.clone()).is_err() {
                store.close().await;
                return Err(ScheduleError::new(
                    "internal_error",
                    "Schedule storage was initialized concurrently.",
                ));
            }
            Ok(store)
        })
        .await
    }

    pub(crate) fn changed(&self) {
        self.ctx.emit(
            "schedule/changed",
            vec![arc(json!({"enabled": self.enabled()}))],
        );
    }

    pub async fn enable(self: &Arc<Self>, config: Config) -> Result<()> {
        config.validate()?;
        let _lifecycle = self.lifecycle.lock().await;
        self.available()?;
        self.stop_requested.store(false, Ordering::SeqCst);
        self.store().await?;
        if self.stop_requested.load(Ordering::SeqCst) {
            return Err(ScheduleError::new(
                "cancelled",
                "Reminder activation was cancelled.",
            ));
        }
        *self.config.write() = config;
        if !self.enabled.swap(true, Ordering::SeqCst) {
            let service = self.clone();
            let generation = self.generation.subscribe();
            *self.driver.lock() = Some(tokio::spawn(async move {
                crate::host_driver::run(service, generation).await;
            }));
        }
        self.changed();
        self.request_drive();
        Ok(())
    }

    pub async fn disable(self: &Arc<Self>) {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.enabled.store(false, Ordering::SeqCst);
        self.generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        self.wake.notify_waiters();
        let _lifecycle = self.lifecycle.lock().await;
        self.disable_locked().await;
    }

    async fn disable_locked(self: &Arc<Self>) {
        self.enabled.store(false, Ordering::SeqCst);
        self.generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        self.wake.notify_waiters();
        let driver = self.driver.lock().take();
        if let Some(driver) = driver {
            let _ = driver.await;
        }
        let _drained = self.gate.clone().lock_owned().await;
        self.changed();
    }

    pub async fn shutdown(self: &Arc<Self>) {
        let _lifecycle = self.lifecycle.lock().await;
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.disable_locked().await;
        let _guard = self.gate.clone().lock_owned().await;
        if let Some(store) = self.store.get() {
            store.close().await;
        }
    }

    async fn read_guard(self: &Arc<Self>) -> Result<(Arc<HostStore>, OwnedMutexGuard<()>)> {
        let store = self.store().await?;
        let guard = self.gate.clone().lock_owned().await;
        self.available()?;
        Ok((store, guard))
    }

    pub async fn catalog(self: &Arc<Self>) -> Result<Vec<Value>> {
        let (store, _guard) = self.read_guard().await?;
        let mut result = Vec::new();
        for item in store.snapshot() {
            if let Some(task) = store.read(&item.id)? {
                let mut value = serde_json::to_value(&task.record).map_err(internal)?;
                let fields = value.as_object_mut().expect("record is an object");
                fields.insert("sessionId".into(), json!(task.session_id));
                fields.insert("status".into(), json!(task.status));
                if let Some(receipt) = task.last_delivery {
                    fields.insert("lastDelivery".into(), json!(receipt));
                }
                result.push(value);
            }
        }
        Ok(result)
    }

    pub async fn list(self: &Arc<Self>, session_id: &str) -> Result<Vec<HostScheduleRecord>> {
        identity(session_id)?;
        let (store, _guard) = self.read_guard().await?;
        store
            .active_for_session(session_id)
            .into_iter()
            .filter_map(|index| match store.read(&index.id) {
                Ok(Some(task)) => Some(Ok(task.record)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub async fn get_task(
        self: &Arc<Self>,
        session_id: &str,
        id: &str,
    ) -> Result<Option<HostScheduleTask>> {
        identity(session_id)?;
        identity(id)?;
        let (store, _guard) = self.read_guard().await?;
        Ok(store.read(id)?.filter(|task| task.session_id == session_id))
    }

    pub async fn session_activity(self: &Arc<Self>, session_id: &str) -> Result<Vec<Value>> {
        Ok(self
            .list(session_id)
            .await?
            .into_iter()
            .map(|record| json!({"id": record.id(), "label": record.title()}))
            .collect())
    }

    pub async fn create(
        self: &Arc<Self>,
        session_id: &str,
        request: ScheduleCreateRequest,
    ) -> Result<HostScheduleRecord> {
        self.create_with_signal(session_id, request, Arc::new(|| false))
            .await
    }

    pub async fn create_with_signal(
        self: &Arc<Self>,
        session_id: &str,
        request: ScheduleCreateRequest,
        cancelled: Cancelled,
    ) -> Result<HostScheduleRecord> {
        self.writable()?;
        identity(session_id)?;
        check_cancelled(&cancelled)?;
        // Relative creation anchors are sampled before admission and FIFO waiting.
        let record = create_record(
            &format!("schedule-{}", uuid::Uuid::new_v4()),
            &request,
            (self.clock)(),
        )?;
        let store = self.store().await?;
        let admission = self.controller.acquire(session_id, false).await?;
        check_cancelled(&cancelled)?;
        let guard = self.gate.clone().lock_owned().await;
        self.writable()?;
        check_cancelled(&cancelled)?;
        let service = self.clone();
        let task = HostScheduleTask {
            session_id: session_id.to_owned(),
            record,
            status: TaskStatus::Active,
            last_delivery: None,
            delivery_history: Some(DeliveryHistory::empty()),
        };
        accepted(async move {
            let _guard = guard;
            store.put(&task).await?;
            service.changed();
            service.request_drive();
            drop(admission);
            Ok(task.record)
        })
        .await
    }

    pub async fn update(
        self: &Arc<Self>,
        request: ScheduleUpdateRequest,
    ) -> Result<ScheduleUpdateResult> {
        self.update_with_signal(request, Arc::new(|| false)).await
    }

    pub async fn update_with_signal(
        self: &Arc<Self>,
        request: ScheduleUpdateRequest,
        cancelled: Cancelled,
    ) -> Result<ScheduleUpdateResult> {
        self.writable()?;
        identity(&request.session_id)?;
        identity(&request.id)?;
        check_cancelled(&cancelled)?;
        let store = self.store().await?;
        let admission = self.controller.acquire(&request.session_id, false).await?;
        check_cancelled(&cancelled)?;
        let guard = self.gate.clone().lock_owned().await;
        self.writable()?;
        check_cancelled(&cancelled)?;
        let service = self.clone();
        accepted(async move {
            let _guard = guard;
            let Some(mut task) = store
                .read(&request.id)?
                .filter(|task| task.session_id == request.session_id)
            else {
                return Ok(update_miss(&request.id, "schedule_not_found"));
            };
            if task.status != TaskStatus::Active {
                return Ok(update_miss(&request.id, "schedule_ended"));
            }
            let result = resolve_update(&task.record, &request, (service.clock)())?;
            if let ScheduleUpdateResult::Changed {
                updated: true,
                record,
                ..
            } = &result
            {
                task.record = record.clone();
                store.put(&task).await?;
                service.changed();
                service.request_drive();
            }
            drop(admission);
            Ok(result)
        })
        .await
    }

    pub async fn delete(self: &Arc<Self>, session_id: &str, id: &str) -> Result<Value> {
        self.delete_with_signal(session_id, id, Arc::new(|| false))
            .await
    }

    pub async fn delete_with_signal(
        self: &Arc<Self>,
        session_id: &str,
        id: &str,
        cancelled: Cancelled,
    ) -> Result<Value> {
        identity(session_id)?;
        identity(id)?;
        check_cancelled(&cancelled)?;
        let (store, guard) = self.read_guard().await?;
        check_cancelled(&cancelled)?;
        let service = self.clone();
        let session_id = session_id.to_owned();
        let id = id.to_owned();
        accepted(async move {
            let _guard = guard;
            let exists = store
                .read(&id)?
                .is_some_and(|task| task.session_id == session_id);
            let deleted = exists && store.delete(&id).await?;
            if deleted {
                service.changed();
                service.request_drive();
            }
            Ok(if deleted {
                json!({"id": id, "deleted": true})
            } else {
                json!({"id": id, "deleted": false, "code": "schedule_not_found"})
            })
        })
        .await
    }

    pub async fn history(self: &Arc<Self>, request: HistoryRequest) -> Result<HistoryResult> {
        identity(&request.session_id)?;
        identity(&request.id)?;
        if !(1..=100).contains(&request.limit) {
            return Err(ScheduleError::invalid(
                "History limit must be an integer between 1 and 100.",
            ));
        }
        if let Some(before) = &request.before {
            identity(before)?;
        }
        let (store, _guard) = self.read_guard().await?;
        let Some(task) = store
            .read(&request.id)?
            .filter(|task| task.session_id == request.session_id)
        else {
            return Ok(HistoryResult::Miss {
                id: request.id,
                code: "schedule_not_found".into(),
            });
        };
        history_page(&task, &request, &self.config().retention())
    }

    /// Called after the session lifecycle owner acquires its admission guard.
    pub async fn stop_session_tasks(self: &Arc<Self>, session_id: &str) -> Result<()> {
        self.remove_session(session_id, true).await
    }
    /// Permanent deletion also removes inactive receipts; no legacy log is imported.
    pub async fn purge_session(self: &Arc<Self>, session_id: &str) -> Result<()> {
        self.remove_session(session_id, false).await
    }
    async fn remove_session(self: &Arc<Self>, session_id: &str, active_only: bool) -> Result<()> {
        identity(session_id)?;
        let (store, guard) = self.read_guard().await?;
        let service = self.clone();
        let session_id = session_id.to_owned();
        accepted(async move {
            let _guard = guard;
            let mut changed = false;
            let outcome = async {
                for index in store.snapshot().into_iter().filter(|index| {
                    index.session_id == session_id && (!active_only || index.active)
                }) {
                    changed |= store.delete(&index.id).await?;
                }
                Ok(())
            }
            .await;
            if changed {
                service.changed();
                service.request_drive();
            }
            outcome
        })
        .await
    }
}

fn identity(value: &str) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(ScheduleError::invalid(
            "A non-empty, trimmed identity is required.",
        ));
    }
    Ok(())
}
fn check_cancelled(cancelled: &Cancelled) -> Result<()> {
    if cancelled() {
        return Err(ScheduleError::new(
            "cancelled",
            "Reminder operation was cancelled before acceptance.",
        ));
    }
    Ok(())
}
fn update_miss(id: &str, code: &str) -> ScheduleUpdateResult {
    ScheduleUpdateResult::Miss {
        id: id.to_owned(),
        updated: false,
        code: code.to_owned(),
    }
}
pub(crate) fn internal(error: impl std::fmt::Display) -> ScheduleError {
    ScheduleError::new("internal_error", error.to_string())
}
pub(crate) async fn accepted<T, F>(future: F) -> Result<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
{
    tokio::spawn(future).await.map_err(internal)?
}
