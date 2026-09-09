use super::*;
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget,
};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, UserMessage, session_id};
use futures::future::BoxFuture;

struct Owner {
    id: SessionId,
    running: AtomicBool,
}

impl Agent for Owner {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn status(&self) -> AgentStatus {
        if self.running.load(Ordering::SeqCst) {
            AgentStatus::Running
        } else {
            AgentStatus::Idle
        }
    }
    fn options(&self) -> &AgentOptions {
        unreachable!()
    }
    fn session(&self) -> &Session {
        unreachable!()
    }
    fn inbox(&self) -> &Inbox {
        unreachable!()
    }
    fn ctx(&self) -> &Context {
        unreachable!()
    }
    fn scope_key(&self) -> &ScopeKey {
        unreachable!()
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

#[derive(Default)]
struct Driver {
    marks: SyncMutex<Vec<String>>,
    calls: SyncMutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ComputerUseAdapter for Driver {
    fn adapter_id(&self) -> &'static str {
        "fixture-desktop"
    }
    fn has_owner_activity(&self, _: &str) -> bool {
        true
    }
    fn mark_owner_active(&self, owner: &str) {
        self.marks.lock().push(owner.into());
    }
    async fn execute(
        &self,
        request: AdapterRequest,
        _: AbortPredicate,
    ) -> Result<AdapterOutput, AdapterError> {
        self.calls.lock().push(request.action);
        Ok(AdapterOutput::json(json!({})))
    }
}

fn remember(runtime: &ComputerUseRuntime, name: &str, running: bool) -> Arc<Owner> {
    let owner = Arc::new(Owner {
        id: session_id(name),
        running: AtomicBool::new(running),
    });
    let erased: Arc<dyn Agent> = owner.clone();
    runtime
        .owner_agents
        .lock()
        .insert(name.into(), Arc::downgrade(&erased));
    owner
}

#[tokio::test]
async fn reaper_retains_only_live_running_owners_without_resuming_manual_control() {
    let driver = Arc::new(Driver::default());
    let runtime = ComputerUseRuntime {
        ctx: Context::root(),
        adapter: Arc::new(control::ControlledAdapter::new(driver.clone())),
        timeout: Duration::from_secs(1),
        owner_agents: SyncMutex::new(HashMap::new()),
    };
    let running = remember(&runtime, "running", true);
    let idle = remember(&runtime, "idle", false);
    drop(remember(&runtime, "disposed", true));
    runtime
        .execute("running", &json!({"action":"capture"}), Arc::new(|| false))
        .await
        .unwrap();
    runtime
        .execute_for_human_session(
            "running".into(),
            &json!({"action":"takeover"}),
            Arc::new(|| false),
        )
        .await
        .unwrap();
    let calls_before = driver.calls.lock().clone();
    runtime.reap_inactive().await;
    assert_eq!(*driver.marks.lock(), vec!["running"]);
    assert_eq!(*driver.calls.lock(), calls_before, "no transport heartbeat");
    let error = runtime
        .execute("running", &json!({"action":"capture"}), Arc::new(|| false))
        .await
        .unwrap_err();
    assert_eq!(error.code, "COMPUTER_USE_MANUAL_CONTROL");
    assert_eq!(*driver.calls.lock(), calls_before);

    running.running.store(false, Ordering::SeqCst);
    idle.running.store(true, Ordering::SeqCst);
    driver.marks.lock().clear();
    runtime.reap_inactive().await;
    assert_eq!(*driver.marks.lock(), vec!["idle"]);
}
