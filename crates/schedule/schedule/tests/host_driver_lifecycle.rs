use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use std::time::Duration;

use cordis::Context;
use dsh_llm::UserMessage;
use dsh_schedule::calendar::{decode_create_request, parse_instant};
use dsh_schedule::host_service::{
    Config, ScheduleService, ScheduleSessionController, ScheduleSessionLease,
};
use dsh_schedule::host_types::{HostScheduleTask, ScheduleError, TaskStatus};
use dsh_storage::{Storage, StorageBackend};
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_json::JsonStorageBackend;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, Semaphore};

#[derive(Clone)]
struct Controller(Arc<ControllerState>);
struct ControllerState {
    gates: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    messages: Mutex<Vec<(String, UserMessage)>>,
    attempts: AtomicUsize,
    warm: AtomicUsize,
    active_warm: AtomicUsize,
    fail: AtomicBool,
    block: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}
impl Controller {
    fn new() -> Self {
        Self(Arc::new(ControllerState {
            gates: Mutex::new(HashMap::new()),
            messages: Mutex::new(Vec::new()),
            attempts: AtomicUsize::new(0),
            warm: AtomicUsize::new(0),
            active_warm: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            block: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }))
    }
    fn gate(&self, session: &str) -> Arc<AsyncMutex<()>> {
        self.0
            .gates
            .lock()
            .entry(session.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}
struct Lease {
    owner: Controller,
    session: String,
    warm: bool,
    _admission: OwnedMutexGuard<()>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.warm {
            self.owner.0.active_warm.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
#[async_trait::async_trait]
impl ScheduleSessionController for Controller {
    async fn acquire(
        &self,
        session: &str,
        wake: bool,
    ) -> Result<Box<dyn ScheduleSessionLease>, ScheduleError> {
        if wake {
            self.0.warm.fetch_add(1, Ordering::SeqCst);
        }
        let admission = self.gate(session).lock_owned().await;
        if wake {
            self.0.active_warm.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Box::new(Lease {
            owner: self.clone(),
            session: session.to_owned(),
            warm: wake,
            _admission: admission,
        }))
    }
}
#[async_trait::async_trait]
impl ScheduleSessionLease for Lease {
    async fn deliver(&self, message: UserMessage) -> Result<(), ScheduleError> {
        assert!(self.warm, "cold management must never enqueue messages");
        self.owner.0.attempts.fetch_add(1, Ordering::SeqCst);
        self.owner.0.entered.add_permits(1);
        if self.owner.0.block.load(Ordering::SeqCst) {
            self.owner.0.release.acquire().await.unwrap().forget();
        }
        if self.owner.0.fail.load(Ordering::SeqCst) {
            return Err(ScheduleError::new(
                "persistence_failed",
                "No durable acknowledgment.",
            ));
        }
        self.owner
            .0
            .messages
            .lock()
            .push((self.session.clone(), message));
        Ok(())
    }
}

struct Fixture {
    ctx: Context,
    service: Arc<ScheduleService>,
    controller: Controller,
    clock: Arc<AtomicI64>,
    backend: Arc<JsonStorageBackend>,
    path: PathBuf,
}

#[tokio::test]
async fn disabled_configuration_changes_notify_clients_without_loading_tasks_or_enabling() {
    let fixture = Fixture::new();
    let notices = Arc::new(AtomicUsize::new(0));
    let stop = fixture
        .ctx
        .on(
            "schedule/changed",
            Arc::new({
                let notices = notices.clone();
                move |_, values| {
                    let notices = notices.clone();
                    Box::pin(async move {
                        let value = cordis::downcast::<Value>(&values[0]).unwrap();
                        assert_eq!(value["enabled"], false);
                        notices.fetch_add(1, Ordering::SeqCst);
                        None
                    })
                }
            }),
            cordis::EventOptions::default(),
        )
        .await;
    fixture.service.configure(Config::default()).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(notices.load(Ordering::SeqCst), 0);
    fixture
        .service
        .configure(Config {
            delivery_history_days: 7,
            delivery_history_records: 2,
        })
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(notices.load(Ordering::SeqCst), 1);
    assert!(!fixture.service.enabled());
    assert_eq!(fixture.service.diagnostics()["storeLoaded"], false);
    stop().await;
    fixture.finish().await;
}
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("schedule-host-driver-{}", uuid::Uuid::new_v4()));
        Self::open(path, Arc::new(AtomicI64::new(1_800_000_000_000)))
    }
    fn open(path: PathBuf, clock: Arc<AtomicI64>) -> Self {
        std::fs::create_dir_all(&path).unwrap();
        let ctx = Context::root();
        let storage = Storage::install(&ctx);
        let backend = JsonStorageBackend::new(path.to_string_lossy());
        storage.backend.register("json", backend.clone()).unwrap();
        DomainFacility::install(
            &ctx,
            DomainFacilityConfig {
                backend: "json".into(),
                routes: Default::default(),
            },
        )
        .unwrap();
        let controller = Controller::new();
        let service = ScheduleService::install_with_clock(
            &ctx,
            Arc::new(controller.clone()),
            Arc::new({
                let clock = clock.clone();
                move || clock.load(Ordering::SeqCst)
            }),
        );
        Self {
            ctx,
            service,
            controller,
            clock,
            backend,
            path,
        }
    }
    async fn create(&self, fields: Value) -> String {
        self.service
            .create("root", decode_create_request(&fields).unwrap())
            .await
            .unwrap()
            .id()
            .to_owned()
    }
    async fn task(&self, id: &str) -> HostScheduleTask {
        self.service.get_task("root", id).await.unwrap().unwrap()
    }
    async fn await_receipts(&self, ids: &[&str]) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut all = true;
                for id in ids {
                    all &= self.task(id).await.last_delivery.is_some();
                }
                if all {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("reminder did not publish its durable receipts");
    }
    async fn close(&self) {
        self.service.shutdown().await;
        self.ctx.fiber.dispose().await;
        self.backend.close().await.unwrap();
    }
    async fn finish(self) {
        self.close().await;
        assert!(self.path.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(self.path).unwrap();
    }
}

#[tokio::test]
async fn disabled_storage_stays_cold_and_restart_delivers_only_latest_recurring_batch() {
    let first = Fixture::new();
    assert!(!first.service.enabled());
    let rejected = first
        .service
        .create(
            "root",
            decode_create_request(
                &json!({"title":"Disabled", "prompt":"Do not run", "after_seconds":1}),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "schedule_disabled");
    first.service.enable(Config::default()).await.unwrap();
    let once = first
        .create(json!({"title":"Once", "prompt":"Once content", "after_seconds":1}))
        .await;
    let every = first
        .create(
            json!({"title":"Every minute", "prompt":"First recurring content", "every_seconds":60}),
        )
        .await;
    let slower = first.create(json!({"title":"Every 90 seconds", "prompt":"Second recurring content", "every_seconds":90})).await;
    assert_eq!(first.controller.0.warm.load(Ordering::SeqCst), 0);
    first.service.disable().await;
    let start = first.clock.load(Ordering::SeqCst);
    first.clock.store(start + 200_000, Ordering::SeqCst);
    first.close().await;

    let second = Fixture::open(first.path.clone(), first.clock.clone());
    assert_eq!(second.service.catalog().await.unwrap().len(), 3);
    assert_eq!(second.controller.0.warm.load(Ordering::SeqCst), 0);
    second.service.enable(Config::default()).await.unwrap();
    second.await_receipts(&[&once, &every, &slower]).await;
    let first_recurring = second.task(&every).await;
    let second_recurring = second.task(&slower).await;
    assert_eq!(second.task(&once).await.status, TaskStatus::Inactive);
    assert_eq!(
        first_recurring.last_delivery.as_ref().unwrap().message_id,
        second_recurring.last_delivery.as_ref().unwrap().message_id
    );
    assert_eq!(
        parse_instant(&first_recurring.last_delivery.as_ref().unwrap().scheduled_at).unwrap(),
        start + 180_000
    );
    assert_eq!(
        parse_instant(first_recurring.record.scheduled_at()).unwrap(),
        start + 240_000
    );
    assert_eq!(
        parse_instant(second_recurring.record.scheduled_at()).unwrap(),
        start + 270_000
    );
    assert_eq!(second.controller.0.messages.lock().len(), 2);
    assert_eq!(second.controller.0.active_warm.load(Ordering::SeqCst), 0);
    for _ in 0..10 {
        second.service.request_drive();
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(second.controller.0.messages.lock().len(), 2);
    second.close().await;

    let third = Fixture::open(second.path.clone(), second.clock.clone());
    third.service.enable(Config::default()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(third.controller.0.warm.load(Ordering::SeqCst), 0);
    assert_eq!(third.task(&once).await.status, TaskStatus::Inactive);
    third.finish().await;
}

#[tokio::test]
async fn disabling_drains_an_accepted_delivery_and_publishes_its_receipt() {
    let fixture = Fixture::new();
    fixture.service.enable(Config::default()).await.unwrap();
    let id = fixture
        .create(
            json!({"title":"Drain", "prompt":"Finish the accepted inbox write", "after_seconds":1}),
        )
        .await;
    fixture.controller.0.block.store(true, Ordering::SeqCst);
    fixture.clock.fetch_add(2_000, Ordering::SeqCst);
    fixture.service.request_drive();
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.controller.0.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let service = fixture.service.clone();
    let disable = tokio::spawn(async move {
        service.disable().await;
    });
    tokio::task::yield_now().await;
    assert!(!fixture.service.enabled());
    assert!(!disable.is_finished());
    fixture.controller.0.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), disable)
        .await
        .unwrap()
        .unwrap();
    let task = fixture.task(&id).await;
    assert_eq!(task.status, TaskStatus::Inactive);
    assert!(task.last_delivery.is_some());
    assert_eq!(fixture.controller.0.messages.lock().len(), 1);
    assert_eq!(fixture.controller.0.active_warm.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn an_unacknowledged_delivery_keeps_its_target_without_busy_retrying() {
    let fixture = Fixture::new();
    fixture.service.enable(Config::default()).await.unwrap();
    let id = fixture.create(json!({"title":"Failure", "prompt":"Keep the task until acknowledged", "after_seconds":1})).await;
    fixture.controller.0.fail.store(true, Ordering::SeqCst);
    fixture.clock.fetch_add(2_000, Ordering::SeqCst);
    fixture.service.request_drive();
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.controller.0.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let task = fixture.task(&id).await;
    assert_eq!(task.status, TaskStatus::Active);
    assert!(task.last_delivery.is_none());
    let attempts = fixture.controller.0.attempts.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        fixture.controller.0.attempts.load(Ordering::SeqCst),
        attempts
    );
    fixture.controller.0.fail.store(false, Ordering::SeqCst);
    fixture.service.request_drive();
    fixture.await_receipts(&[&id]).await;
    assert_eq!(fixture.controller.0.messages.lock().len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn disable_cancels_a_cold_resume_wait_and_cancelled_management_does_not_write() {
    let fixture = Fixture::new();
    fixture.service.enable(Config::default()).await.unwrap();
    let id = fixture
        .create(json!({"title":"Wait", "prompt":"Do not outlive disable", "after_seconds":1}))
        .await;
    let admission = fixture.controller.gate("root").lock_owned().await;
    fixture.clock.fetch_add(2_000, Ordering::SeqCst);
    fixture.service.request_drive();
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.controller.0.warm.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), fixture.service.disable())
        .await
        .unwrap();
    drop(admission);
    assert_eq!(fixture.controller.0.attempts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.task(&id).await.status, TaskStatus::Active);
    fixture.service.delete("root", &id).await.unwrap();
    fixture.service.enable(Config::default()).await.unwrap();
    let admission = fixture.controller.gate("root").lock_owned().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let operation = tokio::spawn({
        let service = fixture.service.clone();
        let cancelled = cancelled.clone();
        async move {
            service
                .create_with_signal(
                    "root",
                    decode_create_request(
                        &json!({"title":"Cancelled", "prompt":"Never persist", "after_seconds":60}),
                    )
                    .unwrap(),
                    Arc::new(move || cancelled.load(Ordering::SeqCst)),
                )
                .await
        }
    });
    tokio::task::yield_now().await;
    cancelled.store(true, Ordering::SeqCst);
    drop(admission);
    assert_eq!(operation.await.unwrap().unwrap_err().code, "cancelled");
    assert!(fixture.service.catalog().await.unwrap().is_empty());
    fixture.finish().await;
}
