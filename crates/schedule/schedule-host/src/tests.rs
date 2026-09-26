use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use futures::future::BoxFuture;
use parking_lot::Mutex;

use crate::*;

struct Clock(Mutex<DateTime<Utc>>);

impl Clock {
    fn advance(&self, by: Duration) {
        let mut now = self.0.lock();
        *now += by;
    }
}

#[derive(Default)]
struct Inbox {
    sent: Mutex<Vec<(String, String)>>,
    fail: Mutex<Option<String>>,
}

impl Deliver for Inbox {
    fn deliver(
        &self,
        session_id: String,
        text: String,
    ) -> BoxFuture<'static, Result<Option<String>, String>> {
        let failure = self.fail.lock().clone();
        let mut sent = self.sent.lock();
        let id = format!("msg-{}", sent.len() + 1);
        if failure.is_none() {
            sent.push((session_id, text));
        }
        Box::pin(async move {
            match failure {
                Some(error) => Err(error),
                None => Ok(Some(id)),
            }
        })
    }
}

struct Fixture {
    dir: std::path::PathBuf,
    clock: Arc<Clock>,
    inbox: Arc<Inbox>,
    service: Arc<ScheduleService>,
}

fn start() -> DateTime<Utc> {
    rules::parse_instant("2026-09-26T08:00:00Z").unwrap()
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dsh-schedule-host-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let clock = Arc::new(Clock(Mutex::new(start())));
        let inbox = Arc::new(Inbox::default());
        let service = Self::open_service(&dir, &clock, ServiceConfig::default());
        service.set_deliver(inbox.clone());
        Self {
            dir,
            clock,
            inbox,
            service,
        }
    }

    fn open_service(
        dir: &std::path::Path,
        clock: &Arc<Clock>,
        config: ServiceConfig,
    ) -> Arc<ScheduleService> {
        let clock = clock.clone();
        ScheduleService::open_with_clock(
            dir.join("schedule.json"),
            config,
            Arc::new(move || *clock.0.lock()),
        )
    }

    fn reopen(&self) -> Arc<ScheduleService> {
        Self::open_service(&self.dir, &self.clock, ServiceConfig::default())
    }

    async fn create(&self, rule: TaskRule) -> TaskView {
        self.service
            .create(CreateTask {
                session_id: "session-a".into(),
                title: None,
                prompt: "总结今天的新闻\n并列出三条要点".into(),
                rule,
                origin: TaskOrigin::User,
            })
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn every(seconds: u64) -> TaskRule {
    TaskRule::Every {
        every_seconds: seconds,
        anchor: String::new(),
    }
}

#[tokio::test]
async fn created_tasks_persist_across_restart() {
    let fixture = Fixture::new("persist");
    let view = fixture.create(every(3600)).await;
    assert_eq!(view.title, "总结今天的新闻");
    assert_eq!(view.status, TaskStatus::Active);
    assert_eq!(
        view.next_run_at.as_deref(),
        Some("2026-09-26T09:00:00.000Z")
    );
    let reopened = fixture.reopen();
    assert!(reopened.load_error().is_none());
    let task = reopened.task(&view.id).unwrap();
    assert_eq!(task.rule, fixture.service.task(&view.id).unwrap().rule);
    assert_eq!(task.next_run_at, view.next_run_at);
}

#[tokio::test]
async fn due_recurring_tasks_deliver_once_and_skip_missed_occurrences() {
    let fixture = Fixture::new("catchup");
    let view = fixture.create(every(3600)).await;
    assert_eq!(
        fixture
            .service
            .drive_once()
            .await
            .map(rules::format_instant)
            .as_deref(),
        Some("2026-09-26T09:00:00.000Z")
    );
    assert!(fixture.inbox.sent.lock().is_empty());

    fixture
        .clock
        .advance(Duration::hours(5) + Duration::minutes(10));
    fixture.service.drive_once().await;
    let sent = fixture.inbox.sent.lock().clone();
    assert_eq!(sent.len(), 1, "one delivery for five missed hours");
    assert_eq!(sent[0].0, "session-a");
    assert!(sent[0].1.starts_with("[SCHEDULED TASK]"));
    assert!(sent[0].1.contains(&format!("schedule_id: {}", view.id)));
    assert!(
        sent[0]
            .1
            .contains("task_prompt_json: \"总结今天的新闻\\n并列出三条要点\"")
    );
    let task = fixture.service.task(&view.id).unwrap();
    assert_eq!(
        task.next_run_at.as_deref(),
        Some("2026-09-26T14:00:00.000Z")
    );
    assert_eq!(task.delivery_history.len(), 1);
    let delivery = task.last_delivery.unwrap();
    assert_eq!(delivery.outcome, DeliveryOutcome::Delivered);
    assert_eq!(delivery.message_id.as_deref(), Some("msg-1"));
    assert_eq!(delivery.occurrence_at, "2026-09-26T09:00:00.000Z");

    // Delivered state survives restart and does not repeat.
    let reopened = fixture.reopen();
    reopened.set_deliver(fixture.inbox.clone());
    reopened.drive_once().await;
    assert_eq!(fixture.inbox.sent.lock().len(), 1);
}

#[tokio::test]
async fn one_shot_tasks_become_inactive_after_delivery_or_failure() {
    let fixture = Fixture::new("oneshot");
    let delivered = fixture
        .create(TaskRule::At {
            at: "2026-09-26T08:30:00Z".into(),
        })
        .await;
    *fixture.inbox.fail.lock() = None;
    fixture.clock.advance(Duration::minutes(31));
    fixture.service.drive_once().await;
    let task = fixture.service.task(&delivered.id).unwrap();
    assert_eq!(task.status, TaskStatus::Inactive);
    assert_eq!(task.next_run_at, None);

    let failing = fixture.create(every(120)).await;
    *fixture.inbox.fail.lock() = Some("会话不存在".into());
    fixture.clock.advance(Duration::minutes(3));
    fixture.service.drive_once().await;
    let task = fixture.service.task(&failing.id).unwrap();
    let delivery = task.last_delivery.unwrap();
    assert_eq!(delivery.outcome, DeliveryOutcome::Failed);
    assert_eq!(delivery.error.as_deref(), Some("会话不存在"));
    assert_eq!(
        task.status,
        TaskStatus::Active,
        "a recurring task keeps its schedule after a failure"
    );
    assert!(task.next_run_at.is_some());

    assert_eq!(
        fixture
            .service
            .create(CreateTask {
                session_id: "session-a".into(),
                title: None,
                prompt: "late".into(),
                rule: TaskRule::At {
                    at: "2026-09-26T08:00:00Z".into()
                },
                origin: TaskOrigin::User,
            })
            .await
            .unwrap_err()
            .code,
        "not_future"
    );
}

#[tokio::test]
async fn updates_detect_stale_drafts_and_recompute_the_next_run() {
    let fixture = Fixture::new("update");
    let view = fixture.create(every(3600)).await;
    let changed = fixture
        .service
        .update(
            &view.id,
            Some("session-a"),
            Some(&view.updated_at),
            UpdateTask {
                title: Some("晨报".into()),
                prompt: None,
                rule: Some(TaskRule::Daily {
                    time: "09:30".into(),
                    time_zone: "Asia/Shanghai".into(),
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(changed.title, "晨报");
    assert_eq!(
        changed.next_run_at.as_deref(),
        Some("2026-09-27T01:30:00.000Z")
    );
    assert_ne!(changed.updated_at, view.updated_at);
    let stale = fixture
        .service
        .update(
            &view.id,
            Some("session-a"),
            Some(&view.updated_at),
            UpdateTask {
                title: Some("x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "conflict");
    let foreign = fixture
        .service
        .update(
            &view.id,
            Some("session-b"),
            None,
            UpdateTask {
                title: Some("x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(foreign.code, "schedule_not_found");
}

#[tokio::test]
async fn disabling_stops_delivery_and_enabling_resumes_from_now() {
    let fixture = Fixture::new("toggle");
    let view = fixture.create(every(600)).await;
    let disabled = fixture
        .service
        .set_active(&view.id, None, false)
        .await
        .unwrap();
    assert_eq!(disabled.status, TaskStatus::Inactive);
    fixture.clock.advance(Duration::hours(2));
    assert_eq!(fixture.service.drive_once().await, None);
    assert!(fixture.inbox.sent.lock().is_empty());
    let enabled = fixture
        .service
        .set_active(&view.id, None, true)
        .await
        .unwrap();
    assert_eq!(enabled.status, TaskStatus::Active);
    assert_eq!(
        enabled.next_run_at.as_deref(),
        Some("2026-09-26T10:10:00.000Z")
    );

    let once = fixture
        .create(TaskRule::At {
            at: "2026-09-26T10:05:00Z".into(),
        })
        .await;
    fixture
        .service
        .set_active(&once.id, None, false)
        .await
        .unwrap();
    fixture.clock.advance(Duration::minutes(30));
    assert_eq!(
        fixture
            .service
            .set_active(&once.id, None, true)
            .await
            .unwrap_err()
            .code,
        "not_future"
    );
}

#[tokio::test]
async fn deletion_removes_the_task_and_its_history() {
    let fixture = Fixture::new("delete");
    let view = fixture.create(every(60)).await;
    fixture.clock.advance(Duration::minutes(2));
    fixture.service.drive_once().await;
    assert_eq!(
        fixture
            .service
            .history(&view.id, None, 0, 10)
            .unwrap()
            .total,
        1
    );
    fixture
        .service
        .delete(&view.id, Some("session-a"))
        .await
        .unwrap();
    assert!(fixture.service.task(&view.id).is_none());
    assert_eq!(
        fixture
            .service
            .history(&view.id, None, 0, 10)
            .unwrap_err()
            .code,
        "schedule_not_found"
    );
    assert!(fixture.reopen().task(&view.id).is_none());
}

#[tokio::test]
async fn damaged_storage_is_reported_and_never_overwritten() {
    let fixture = Fixture::new("corrupt");
    let path = fixture.dir.join("schedule.json");
    std::fs::write(&path, b"{\"version\":1,\"tasks\":[{\"id\":").unwrap();
    let service = fixture.reopen();
    assert!(service.load_error().unwrap().contains("格式无效"));
    let error = service
        .create(CreateTask {
            session_id: "s".into(),
            title: None,
            prompt: "x".into(),
            rule: every(60),
            origin: TaskOrigin::User,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "storage_unavailable");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{\"version\":1,\"tasks\":[{\"id\":"
    );
    assert!(service.catalog(None)["error"].is_string());
}

#[tokio::test]
async fn history_is_bounded_newest_first_and_marks_pruning() {
    let fixture = Fixture::new("history");
    let service = Fixture::open_service(
        &fixture.dir,
        &fixture.clock,
        ServiceConfig {
            history_days: 30,
            history_records: 3,
        },
    );
    service.set_deliver(fixture.inbox.clone());
    let view = service
        .create(CreateTask {
            session_id: "session-a".into(),
            title: Some("心跳".into()),
            prompt: "ping".into(),
            rule: every(60),
            origin: TaskOrigin::Agent,
        })
        .await
        .unwrap();
    for _ in 0..5 {
        fixture.clock.advance(Duration::seconds(61));
        service.drive_once().await;
    }
    let page = service.history(&view.id, None, 0, 2).unwrap();
    assert_eq!(page.total, 3);
    assert!(page.has_more);
    assert!(page.earlier_records_pruned);
    assert_eq!(page.records.len(), 2);
    assert!(page.records[0].delivered_at > page.records[1].delivered_at);
    let rest = service.history(&view.id, None, 2, 2).unwrap();
    assert_eq!(rest.records.len(), 1);
    assert!(!rest.has_more);
    assert!(
        fixture.inbox.sent.lock()[0]
            .1
            .contains("You created this scheduled task")
    );
}

#[tokio::test]
async fn run_now_delivers_without_moving_the_schedule() {
    let fixture = Fixture::new("runnow");
    let view = fixture
        .create(TaskRule::Daily {
            time: "09:00".into(),
            time_zone: "UTC".into(),
        })
        .await;
    let delivery = fixture
        .service
        .run_now(&view.id, Some("session-a"))
        .await
        .unwrap();
    assert!(delivery.manual);
    assert_eq!(delivery.outcome, DeliveryOutcome::Delivered);
    let task = fixture.service.task(&view.id).unwrap();
    assert_eq!(task.next_run_at, view.next_run_at);
    assert_eq!(task.delivery_history.len(), 1);
    assert!(
        fixture.inbox.sent.lock()[0]
            .1
            .starts_with("[SCHEDULED TASK · RUN NOW]")
    );
    assert_eq!(
        fixture
            .service
            .run_now(&view.id, Some("other"))
            .await
            .unwrap_err()
            .code,
        "schedule_not_found"
    );
}

#[tokio::test]
async fn change_waiters_wake_on_mutation() {
    let fixture = Fixture::new("wait");
    let seen = fixture.service.revision();
    let service = fixture.service.clone();
    let waiter =
        tokio::spawn(async move { service.wait_change(seen, StdDuration::from_secs(5)).await });
    tokio::time::sleep(StdDuration::from_millis(20)).await;
    fixture.create(every(60)).await;
    let revision = tokio::time::timeout(StdDuration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revision, seen + 1);
    assert_eq!(
        fixture
            .service
            .wait_change(seen, StdDuration::from_millis(10))
            .await,
        seen + 1
    );
}

#[test]
fn model_selectors_are_exclusive_and_mapped() {
    let service = ScheduleService::open(
        std::env::temp_dir().join("dsh-schedule-selector-unused.json"),
        ServiceConfig::default(),
    );
    let args = serde_json::json!({"prompt": "x", "weekly": {"time": "09:00", "weekdays": [1, 3], "time_zone": "Asia/Shanghai"}});
    assert!(matches!(
        rule_from_args(&args, &service).unwrap(),
        Some(TaskRule::Weekly { .. })
    ));
    let both = serde_json::json!({"at": "2026-09-27T09:00:00+08:00", "every_seconds": 120});
    assert_eq!(
        rule_from_args(&both, &service).unwrap_err().code,
        "invalid_selector"
    );
    assert_eq!(
        rule_from_args(&serde_json::json!({"title": "x"}), &service).unwrap(),
        None
    );
    assert!(rule_from_args(&serde_json::json!({"after_seconds": 30}), &service).is_err());
}
