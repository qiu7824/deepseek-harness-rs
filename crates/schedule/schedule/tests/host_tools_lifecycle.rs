use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use cordis::{BoxFuture, Context};
use dsh_agent::{
    Agent, AgentCancelCause, AgentOptions, AgentRegistry, AgentStatus, CancelOptions, Inbox,
    InboxNotifications, InboxTarget,
};
use dsh_schedule::{
    host_plugin::apply_host_schedule,
    host_service::{Config, ScheduleService, ScheduleSessionController, ScheduleSessionLease},
    host_tools::register_host_schedule_tools,
    host_types::{ScheduleError, TaskStatus},
};
use dsh_scope::{Scope, ScopeKey, create_scope};
use dsh_session::{Session, SessionId, SessionStore, UserMessage, session_id};
use dsh_storage::{Storage, StorageBackend};
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_json::JsonStorageBackend;
use dsh_tools::{ToolDefinition, ToolExecutionInput, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};

struct FixtureAgent {
    scope: Scope,
    key: ScopeKey,
    session: Session,
    inbox: Inbox,
    options: AgentOptions,
}

impl FixtureAgent {
    fn new(ctx: &Context, id: &str, parent: Option<ScopeKey>) -> Arc<Self> {
        let session = Session::create(session_id(id), None, None, None).unwrap();
        let key = ScopeKey::new();
        Arc::new(Self {
            scope: create_scope(ctx, key.clone(), &dsh_scope::CreateScopeOptions { parent }),
            key,
            inbox: Inbox::new(&session, InboxNotifications::default()).unwrap(),
            session,
            options: AgentOptions::default(),
        })
    }
}
impl Agent for FixtureAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn inbox(&self) -> &Inbox {
        &self.inbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.scope.ctx
    }
    fn scope_key(&self) -> &ScopeKey {
        &self.key
    }
    fn cancel(&self, _: AgentCancelCause, _: Option<&CancelOptions>) {}
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        task: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> BoxFuture<'static, ()> {
        task()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) {}
    fn followup(&self, _: UserMessage) {}
    fn steer(&self, _: UserMessage) {}
    fn inject(&self, _: UserMessage) {}
}

#[derive(Default)]
struct Controller {
    cold: AtomicUsize,
    wake: AtomicUsize,
}
struct Lease;
#[async_trait::async_trait]
impl ScheduleSessionController for Controller {
    async fn acquire(
        &self,
        _: &str,
        wake: bool,
    ) -> Result<Box<dyn ScheduleSessionLease>, ScheduleError> {
        if wake {
            self.wake.fetch_add(1, Ordering::SeqCst);
        } else {
            self.cold.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Box::new(Lease))
    }
}
#[async_trait::async_trait]
impl ScheduleSessionLease for Lease {
    async fn deliver(&self, _: UserMessage) -> Result<(), ScheduleError> {
        Ok(())
    }
}

struct Fixture {
    ctx: Context,
    agents: Arc<AgentRegistry>,
    tools: Arc<ToolRuntime>,
    service: Arc<ScheduleService>,
    controller: Arc<Controller>,
    clock: Arc<AtomicI64>,
    backend: Arc<JsonStorageBackend>,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dsh-host-schedule-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        SessionStore::install(&ctx);
        let agents = AgentRegistry::install(&ctx);
        let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
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
        let controller = Arc::new(Controller::default());
        let clock = Arc::new(AtomicI64::new(4_000_000_000_000));
        let service = ScheduleService::install_with_clock(
            &ctx,
            controller.clone(),
            Arc::new({
                let clock = clock.clone();
                move || clock.load(Ordering::SeqCst)
            }),
        );
        Self {
            ctx,
            agents,
            tools,
            service,
            controller,
            clock,
            backend,
            path,
        }
    }
    async fn finish(self) {
        self.ctx.fiber.dispose().await;
        self.backend.close().await.unwrap();
        std::fs::remove_dir_all(self.path).unwrap();
    }
}

fn catalog(tools: &ToolRuntime, agent: &dyn Agent) -> Vec<String> {
    tools
        .schemas(Some(agent.scope_key()))
        .into_iter()
        .filter(|schema| schema.name.starts_with("schedule_"))
        .map(|schema| schema.name)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn host_child_fence_covers_existing_new_and_copied_tools_across_optional_reload() {
    let fixture = Fixture::new();
    let root = FixtureAgent::new(&fixture.ctx, "boundary-root", None);
    let agent: Arc<dyn Agent> = root.clone();
    let root_detach = fixture.agents.enter(agent.clone(), None).unwrap();
    fixture.agents.announce(&agent).await.unwrap();
    let first = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    let child = FixtureAgent::new(&fixture.ctx, "existing-child", Some(root.key.clone()));
    let child_agent: Arc<dyn Agent> = child.clone();
    fixture
        .tools
        .inherit_visible(child.ctx(), &fixture.tools, root.scope_key())
        .unwrap();
    let child_detach = fixture
        .agents
        .enter(child_agent.clone(), Some(agent.clone()))
        .unwrap();
    fixture.agents.announce(&child_agent).await.unwrap();
    assert_eq!(catalog(&fixture.tools, child.as_ref()), expected());
    let boundary = dsh_schedule::host_boundary::install_host_schedule_tool_boundaries(&fixture.ctx)
        .await
        .unwrap();
    assert!(catalog(&fixture.tools, child.as_ref()).is_empty());
    assert_eq!(catalog(&fixture.tools, root.as_ref()), expected());
    let mut optional = Some(first);
    for enabled in [false, true, false] {
        if enabled {
            optional = Some(
                apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
                    .await
                    .unwrap(),
            );
        } else {
            optional.take().unwrap()().await;
        }
        assert!(catalog(&fixture.tools, child.as_ref()).is_empty());
        let result = fixture
            .tools
            .execute(ToolExecutionInput {
                call_id: dsh_llm::call_id(format!("child-denied-{enabled}")),
                root_call_id: None,
                name: "schedule_create".into(),
                arguments: json!({"title":"Forbidden","prompt":"Child","after_seconds":60}),
                agent: Some(child_agent.clone()),
                parent: None,
                signal: Arc::new(|| false),
            })
            .await;
        assert!(
            result.is_error,
            "copied definitions cannot bypass the child fence"
        );
    }
    let late = FixtureAgent::new(&fixture.ctx, "late-child", Some(root.key.clone()));
    let late_agent: Arc<dyn Agent> = late.clone();
    let late_detach = fixture
        .agents
        .enter(late_agent.clone(), Some(agent.clone()))
        .unwrap();
    fixture.agents.announce(&late_agent).await.unwrap();
    fixture
        .tools
        .inherit_visible(late.ctx(), &fixture.tools, root.scope_key())
        .unwrap();
    let last = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    assert!(catalog(&fixture.tools, late.as_ref()).is_empty());
    assert_eq!(catalog(&fixture.tools, root.as_ref()), expected());
    assert!(fixture.service.catalog().await.unwrap().is_empty());
    child_detach().await;
    late_detach().await;
    root_detach().await;
    (child.scope.dispose)().await;
    (late.scope.dispose)().await;
    (root.scope.dispose)().await;
    last().await;
    boundary().await;
    fixture.finish().await;
}
fn expected() -> Vec<String> {
    [
        "schedule_create",
        "schedule_delete",
        "schedule_list",
        "schedule_update",
    ]
    .map(str::to_owned)
    .to_vec()
}
async fn call(fixture: &Fixture, agent: Arc<dyn Agent>, name: &str, arguments: Value) -> Value {
    let result = fixture
        .tools
        .execute(ToolExecutionInput {
            call_id: dsh_llm::call_id(format!("test-{}", uuid::Uuid::new_v4())),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: Some(agent),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await;
    assert!(
        !result.is_error,
        "{}: {:?}",
        name,
        result.error.as_ref().map(|error| &error.message)
    );
    result.value.clone().expect("tool canonical output")
}

#[tokio::test(flavor = "current_thread")]
async fn attaches_existing_and_new_roots_binds_crud_and_rejects_inherited_child_calls() {
    let fixture = Fixture::new();
    let first = FixtureAgent::new(&fixture.ctx, "root-one", None);
    let first_agent: Arc<dyn Agent> = first.clone();
    let first_detach = fixture.agents.enter(first_agent.clone(), None).unwrap();
    fixture.agents.announce(&first_agent).await.unwrap();
    let stop = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    assert_eq!(catalog(&fixture.tools, first.as_ref()), expected());
    let second = FixtureAgent::new(&fixture.ctx, "root-two", None);
    let second_agent: Arc<dyn Agent> = second.clone();
    let second_detach = fixture.agents.enter(second_agent.clone(), None).unwrap();
    fixture.agents.announce(&second_agent).await.unwrap();
    assert_eq!(catalog(&fixture.tools, second.as_ref()), expected());
    assert!(
        fixture
            .tools
            .schemas(None)
            .iter()
            .all(|schema| !schema.name.starts_with("schedule_"))
    );
    let value = call(
        &fixture,
        first_agent.clone(),
        "schedule_create",
        json!({"title":"Check","prompt":"Review the report","every_seconds":60}),
    )
    .await;
    assert_eq!(value["deliveryMode"], "host");
    let id = value["id"].as_str().unwrap();
    let target = value["scheduledAt"].clone();
    assert_eq!(
        call(&fixture, second_agent.clone(), "schedule_list", json!({})).await,
        json!([])
    );
    let wrong = call(
        &fixture,
        second_agent.clone(),
        "schedule_update",
        json!({"id":id,"title":"Wrong owner"}),
    )
    .await;
    assert_eq!(wrong["code"], "schedule_not_found");
    let update = call(
        &fixture,
        first_agent.clone(),
        "schedule_update",
        json!({"id":id,"title":"Revised"}),
    )
    .await;
    assert_eq!(update["title"], "Revised");
    assert_eq!(update["scheduledAt"], target);
    let child = FixtureAgent::new(&fixture.ctx, "child", Some(first.key.clone()));
    let child_agent: Arc<dyn Agent> = child.clone();
    let child_detach = fixture
        .agents
        .enter(child_agent.clone(), Some(first_agent.clone()))
        .unwrap();
    fixture.agents.announce(&child_agent).await.unwrap();
    let result = call(
        &fixture,
        child_agent.clone(),
        "schedule_create",
        json!({"title":"Forbidden","prompt":"Child","after_seconds":60}),
    )
    .await;
    assert_eq!(result["code"], "internal_error");
    assert_eq!(fixture.service.list("root-one").await.unwrap().len(), 1);
    assert_eq!(fixture.service.list("child").await.unwrap().len(), 0);
    assert!(
        register_host_schedule_tools(
            &fixture.ctx,
            child.ctx(),
            child_agent.clone(),
            fixture.service.clone()
        )
        .is_err()
    );
    let removed = call(
        &fixture,
        first_agent.clone(),
        "schedule_delete",
        json!({"id":id}),
    )
    .await;
    assert_eq!(removed, json!({"id":id,"deleted":true}));
    assert_eq!(
        fixture.controller.wake.load(Ordering::SeqCst),
        0,
        "CRUD must never wake an Agent"
    );
    assert!(fixture.controller.cold.load(Ordering::SeqCst) > 0);
    child_detach().await;
    (child.scope.dispose)().await;
    first_detach().await;
    second_detach().await;
    (first.scope.dispose)().await;
    (second.scope.dispose)().await;
    stop().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn immediate_agent_retirement_does_not_block_effect_setup_or_retain_catalogs() {
    let fixture = Fixture::new();
    let stop = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    for index in 0..16 {
        let concrete = FixtureAgent::new(&fixture.ctx, &format!("immediate-{index}"), None);
        let agent: Arc<dyn Agent> = concrete.clone();
        let weak = Arc::downgrade(&concrete);
        let detach = fixture.agents.enter(agent.clone(), None).unwrap();
        fixture.agents.announce(&agent).await.unwrap();
        assert_eq!(catalog(&fixture.tools, agent.as_ref()), expected());
        detach().await;
        drop(detach);
        tokio::time::timeout(Duration::from_secs(5), (concrete.scope.dispose)())
            .await
            .unwrap();
        assert!(catalog(&fixture.tools, agent.as_ref()).is_empty());
        drop(agent);
        drop(concrete);
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retired root retained by schedule");
    }
    stop().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn unloading_retains_tasks_reloading_reattaches_roots_and_old_disposer_is_idempotent() {
    let fixture = Fixture::new();
    let root = FixtureAgent::new(&fixture.ctx, "reload-root", None);
    let agent: Arc<dyn Agent> = root.clone();
    let detach = fixture.agents.enter(agent.clone(), None).unwrap();
    fixture.agents.announce(&agent).await.unwrap();
    let first = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    let created = call(
        &fixture,
        agent.clone(),
        "schedule_create",
        json!({"title":"Durable","prompt":"Continue","after_seconds":60}),
    )
    .await;
    first().await;
    assert!(!fixture.service.enabled());
    assert!(catalog(&fixture.tools, agent.as_ref()).is_empty());
    assert_eq!(fixture.service.list("reload-root").await.unwrap().len(), 1);
    let second = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    assert_eq!(catalog(&fixture.tools, agent.as_ref()), expected());
    first().await;
    assert!(
        fixture.service.enabled(),
        "retired disposer must not disable a new plugin lifetime"
    );
    let id = created["id"].as_str().unwrap();
    fixture.clock.fetch_add(60_000, Ordering::SeqCst);
    fixture.service.request_drive();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture
                .service
                .get_task("reload-root", id)
                .await
                .unwrap()
                .is_some_and(|task| task.status == TaskStatus::Inactive)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("one-shot should finish");
    let ended = call(
        &fixture,
        agent.clone(),
        "schedule_update",
        json!({"id":id,"title":"Ended"}),
    )
    .await;
    assert_eq!(ended["code"], "schedule_ended");
    assert_eq!(
        call(&fixture, agent.clone(), "schedule_delete", json!({"id":id})).await["deleted"],
        true
    );
    detach().await;
    (root.scope.dispose)().await;
    second().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_catalog_registration_rolls_back_earlier_tools_atomically() {
    let fixture = Fixture::new();
    fixture.service.enable(Config::default()).await.unwrap();
    let root = FixtureAgent::new(&fixture.ctx, "collision-root", None);
    let agent: Arc<dyn Agent> = root.clone();
    let detach = fixture.agents.enter(agent.clone(), None).unwrap();
    fixture.agents.announce(&agent).await.unwrap();
    let prior = fixture
        .tools
        .register(
            root.ctx(),
            ToolDefinition {
                name: "schedule_delete".into(),
                description: "Existing registration".into(),
                parameters: json!({"type":"object","properties":{}}),
                output: ToolOutputDefinition {
                    schema: json!({"type":"null"}),
                    render: Arc::new(|_, _| Ok(vec![])),
                    presentation_meta: None,
                },
                timeout_ms: None,
                is_concurrency_safe: None,
                execute: Arc::new(|_, _| Box::pin(async { Ok(Value::Null) })),
                finalize_content: None,
                present_call: None,
                present_result: None,
            },
        )
        .unwrap();
    assert!(
        register_host_schedule_tools(
            &fixture.ctx,
            root.ctx(),
            agent.clone(),
            fixture.service.clone()
        )
        .is_err()
    );
    assert_eq!(
        catalog(&fixture.tools, root.as_ref()),
        vec!["schedule_delete"]
    );
    prior().await;
    detach().await;
    (root.scope.dispose)().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn historical_schedule_events_are_read_only_and_never_populate_host_tasks() {
    let fixture = Fixture::new();
    let root = FixtureAgent::new(&fixture.ctx, "legacy-root", None);
    root.session.append("schedule/change",json!({"version":1,"operation":"create","schedule":{"id":"schedule-1","kind":"after","prompt":"Legacy reminder","afterSeconds":60,"scheduledAt":"2096-10-02T07:07:40.000Z"}}),None).unwrap();
    let original = serde_json::to_value(root.session.own_events().as_ref()).unwrap();
    let agent: Arc<dyn Agent> = root.clone();
    let detach = fixture.agents.enter(agent.clone(), None).unwrap();
    fixture.agents.announce(&agent).await.unwrap();
    let stop = apply_host_schedule(&fixture.ctx, fixture.service.clone(), Config::default())
        .await
        .unwrap();
    assert_eq!(
        call(&fixture, agent.clone(), "schedule_list", json!({})).await,
        json!([])
    );
    assert!(fixture.service.catalog().await.unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(root.session.own_events().as_ref()).unwrap(),
        original
    );
    detach().await;
    (root.scope.dispose)().await;
    stop().await;
    fixture.finish().await;
}
