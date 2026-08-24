//! Rust port of the core `packages/subagent/subagent/tests/continuation.spec.ts`
//! behaviors, exercised through lightweight probes instead of a live agent
//! loop: startContinuable identities and descriptor publication, followup
//! admission and authorization, ownership/teardown, reporting, interrupts,
//! and scoped drain routing.
//!
//! # Deviations
//!
//! - Cold resume is not ported: `followup` on a non-resident child rejects
//!   with `NOT_RESUMABLE` (TS cold-materializes persisted children instead).
//! - Inbox claimed/discarded accounting is not wired: the settlement watcher
//!   observes Agent quiescence (`status` + owned children) only, so an
//!   Activation settles through explicit teardown (`drain` /
//!   `drainDescendants`) in these tests rather than on its own.
//! - The manager is exercised directly (like the TS tests' package-private
//!   `continuations` handle) plus one runtime-level forwarding test.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentFactory, AgentHandle, AgentOptions, AgentRegistry, AgentStatus, CancelOptions,
    CreateAgentOptions, Inbox, InboxTarget, ResumeAgentOptions,
};
use dsh_llm::{ContentBlock, MessageSource};
use dsh_scope::ScopeKey;
use dsh_session::{
    AgentCancelCause, Session, SessionEvent, SessionHeader, SessionId, UserMessage, session_id,
};
use dsh_session_persistence::{
    SessionInspection, SessionPersistenceApi, SessionPersistenceSnapshot, SessionReadFromResult,
    session_persistence_revision,
};
use dsh_subagent::lifecycle::{ActivationObserver, create_activation_observer};
use dsh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, ContinuableStartSpec, ContinuationHost,
    ResolvedSubagentStartRequest, SubagentCapabilities, SubagentContinuationManager, SubagentError,
    SubagentFollowupOptions, SubagentInterruptAuthority, SubagentProvider, SubagentReportDelivery,
    SubagentReportOptions, SubagentRun, SubagentRuntime, SubagentStartRequest, SubagentStopReason,
};

fn never_signal() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

fn text(content: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: content.to_string(),
    }]
}

fn texts(messages: &[UserMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn start_spec(parent: Arc<dyn Agent>) -> ContinuableStartSpec {
    let signal = never_signal();
    ContinuableStartSpec {
        provider: "spawn".to_string(),
        label: "child task".to_string(),
        request: SubagentStartRequest {
            label: Some("child task".to_string()),
            prompt: text("child task"),
            parent,
            signal: signal.clone(),
            agent_options: None,
            output_schema: None,
            max_depth: None,
            tool_filter: None,
            persona: None,
        },
        signal,
    }
}

fn followup_options() -> SubagentFollowupOptions {
    SubagentFollowupOptions {
        source: MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
        signal: never_signal(),
    }
}

/// A probe agent whose inbox deliveries are recorded and whose status is
/// test-controlled.
struct ProbeAgent {
    id: SessionId,
    session: Session,
    scope_key: ScopeKey,
    ctx: Context,
    options: AgentOptions,
    status: parking_lot::Mutex<AgentStatus>,
    followups: parking_lot::Mutex<Vec<UserMessage>>,
    steers: parking_lot::Mutex<Vec<UserMessage>>,
    injections: parking_lot::Mutex<Vec<UserMessage>>,
    cancels: parking_lot::Mutex<Vec<String>>,
}

impl ProbeAgent {
    fn new(id: SessionId, session: Session, options: AgentOptions) -> Arc<Self> {
        Arc::new(Self {
            id,
            session,
            scope_key: ScopeKey::new(),
            ctx: Context::root(),
            options,
            status: parking_lot::Mutex::new(AgentStatus::Idle),
            followups: parking_lot::Mutex::new(Vec::new()),
            steers: parking_lot::Mutex::new(Vec::new()),
            injections: parking_lot::Mutex::new(Vec::new()),
            cancels: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn top(id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Self::new(id, session, AgentOptions::default())
    }

    fn set_status(&self, status: AgentStatus) {
        *self.status.lock() = status;
    }
}

impl Agent for ProbeAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> AgentStatus {
        *self.status.lock()
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, cause: AgentCancelCause, _options: Option<&CancelOptions>) {
        self.cancels.lock().push(format!("{cause:?}"));
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, message: UserMessage) {
        self.followups.lock().push(message);
    }

    fn steer(&self, message: UserMessage) {
        self.steers.lock().push(message);
    }

    fn inject(&self, message: UserMessage) {
        self.injections.lock().push(message);
    }
}

/// An agent factory that builds `ProbeAgent` children from the registry's
/// creation options.
#[derive(Default)]
struct ProbeFactory {
    created: parking_lot::Mutex<HashMap<String, Arc<ProbeAgent>>>,
    error: parking_lot::Mutex<Option<String>>,
}

impl ProbeFactory {
    fn agent(&self, id: &SessionId) -> Arc<ProbeAgent> {
        self.created
            .lock()
            .get(id.as_str())
            .cloned()
            .expect("created probe agent")
    }
}

#[async_trait::async_trait]
impl AgentFactory for ProbeFactory {
    async fn create_agent(
        &self,
        owner_ctx: &Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, String> {
        if let Some(error) = self.error.lock().take() {
            return Err(error);
        }
        let id = options
            .session_id
            .clone()
            .ok_or_else(|| "missing session id".to_string())?;
        let meta = options.meta.clone();
        let header = SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: id.clone(),
            created_at: meta.as_ref().and_then(|meta| meta.created_at).unwrap_or(0),
            cwd: meta.as_ref().and_then(|meta| meta.cwd.clone()),
            parent_session: meta.as_ref().and_then(|meta| meta.parent_session.clone()),
            seed_length: meta.as_ref().and_then(|meta| meta.seed_length),
            origin: meta.as_ref().and_then(|meta| meta.origin.clone()),
            delegation_depth: meta.as_ref().and_then(|meta| meta.delegation_depth),
            agent_preset: meta.as_ref().and_then(|meta| meta.agent_preset.clone()),
        };
        let session = Session::create(id.clone(), options.seed.clone(), Some(&header))?;
        let agent_options = options.agent_options.unwrap_or_default();
        let agent = ProbeAgent::new(id.clone(), session, agent_options);
        // TS factory semantics: creation publishes the agent. Enter and
        // announce it so the exact-live-parent authorization can see it.
        let registry = owner_ctx
            .get_typed::<Arc<AgentRegistry>>("agents", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "agents service missing".to_string())?;
        let detach = registry.enter(agent.clone(), None)?;
        let agent_for_announce: Arc<dyn Agent> = agent.clone();
        registry.announce(&agent_for_announce).await?;
        self.created
            .lock()
            .insert(id.as_str().to_string(), agent.clone());
        let detach_for_dispose = detach.clone();
        Ok(AgentHandle {
            agent,
            dispose: Box::pin(async move {
                detach_for_dispose().await;
            }),
        })
    }

    async fn resume(
        &self,
        _owner_ctx: &Context,
        _options: ResumeAgentOptions,
    ) -> Result<AgentHandle, String> {
        Err("resume is not ported in this probe".to_string())
    }
}

/// The manager-side continuation host seam this test world drives directly.
struct ProbeHost {
    ctx: Context,
    prepares: parking_lot::Mutex<Vec<ContinuableCreateRequest>>,
    prepare_error: parking_lot::Mutex<Option<SubagentError>>,
    seed: parking_lot::Mutex<Option<Vec<SessionEvent>>>,
    abort_on_prepare: parking_lot::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
}

impl ContinuationHost for ProbeHost {
    fn prepare_continuable(
        &self,
        _name: &str,
        request: ContinuableCreateRequest,
    ) -> cordis::BoxFuture<'static, Result<ContinuableCreateSpec, SubagentError>> {
        self.prepares.lock().push(request);
        let error = self.prepare_error.lock().take();
        let seed = self.seed.lock().take();
        let abort = self.abort_on_prepare.lock().clone();
        Box::pin(async move {
            if let Some(abort) = abort {
                abort.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(error) = error {
                return Err(error);
            }
            Ok(ContinuableCreateSpec { seed })
        })
    }

    fn observe_activation(
        &self,
        provider: &str,
        child_id: &SessionId,
        parent: &Arc<dyn Agent>,
    ) -> ActivationObserver {
        create_activation_observer(&self.ctx, provider, child_id, parent.clone())
    }
}

/// A cold-store fake persistence backend.
#[derive(Default)]
struct ColdPersistence {
    entries: HashMap<String, (SessionHeader, Vec<SessionEvent>)>,
}

#[async_trait::async_trait]
impl SessionPersistenceApi for ColdPersistence {
    fn locate(&self, _meta: &SessionHeader) -> Option<dsh_session_persistence::SessionLocation> {
        None
    }

    fn supports_raw_artifacts(&self) -> bool {
        false
    }

    async fn create(&self, _meta: SessionHeader) -> Result<(), String> {
        Ok(())
    }

    async fn append(&self, _id: &SessionId, _events: &[SessionEvent]) -> Result<(), String> {
        Ok(())
    }

    async fn inspect(&self, id: &SessionId) -> Result<SessionInspection, String> {
        let Some((meta, events)) = self.entries.get(id.as_str()) else {
            return Err("missing".to_string());
        };
        Ok(SessionInspection {
            meta: meta.clone(),
            events: events.clone(),
        })
    }

    async fn load(&self, id: &SessionId) -> Result<SessionInspection, String> {
        self.inspect(id).await
    }

    async fn read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<SessionReadFromResult, String> {
        let whole = self.inspect(id).await?;
        Ok(SessionReadFromResult {
            meta: whole.meta,
            events: whole
                .events
                .into_iter()
                .filter(|event| event.seq >= from_seq)
                .collect(),
        })
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, String> {
        Ok(self
            .entries
            .values()
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn list_snapshots(&self) -> Result<Vec<SessionPersistenceSnapshot>, String> {
        Ok(self
            .entries
            .iter()
            .map(|(id, (meta, _))| SessionPersistenceSnapshot {
                header: meta.clone(),
                revision: session_persistence_revision(format!("test:{id}")),
            })
            .collect())
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }
}

/// The assembled test world: registry + factory + persistence + manager.
struct TestWorld {
    ctx: Context,
    registry: Arc<AgentRegistry>,
    factory: Arc<ProbeFactory>,
    host: Arc<ProbeHost>,
    manager: Arc<SubagentContinuationManager>,
    parent: Arc<ProbeAgent>,
}

impl TestWorld {
    async fn start_child(&self) -> dsh_subagent::ContinuableStart {
        self.manager
            .start_continuable(start_spec(self.parent.clone()))
            .await
            .expect("start")
    }

    fn parent_agent(&self) -> Arc<dyn Agent> {
        self.parent.clone()
    }

    async fn register_agent(&self, agent: Arc<ProbeAgent>) -> Arc<dyn Agent> {
        let agent: Arc<dyn Agent> = agent;
        self.registry.enter(agent.clone(), None).expect("enter");
        self.registry.announce(&agent).await.expect("announce");
        agent
    }
}

async fn setup_with(persistence: bool) -> TestWorld {
    let ctx = Context::root();
    let registry = AgentRegistry::install(&ctx);
    let factory = Arc::new(ProbeFactory::default());
    registry.set_factory(factory.clone());
    if persistence {
        let erased: Arc<dyn SessionPersistenceApi> = Arc::new(ColdPersistence::default());
        ctx.register_service(erased);
    }
    let host = Arc::new(ProbeHost {
        ctx: ctx.clone(),
        prepares: parking_lot::Mutex::new(Vec::new()),
        prepare_error: parking_lot::Mutex::new(None),
        seed: parking_lot::Mutex::new(None),
        abort_on_prepare: parking_lot::Mutex::new(None),
    });
    let manager = SubagentContinuationManager::new(&ctx, host.clone());
    let parent = ProbeAgent::top("parent");
    let parent_agent: Arc<dyn Agent> = parent.clone();
    registry
        .enter(parent_agent.clone(), None)
        .expect("enter parent");
    registry
        .announce(&parent_agent)
        .await
        .expect("announce parent");
    TestWorld {
        ctx,
        registry,
        factory,
        host,
        manager,
        parent,
    }
}

async fn setup() -> TestWorld {
    setup_with(true).await
}

// ---- startContinuable ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_returns_identities_and_publishes_the_child() {
    let world = setup().await;
    let started = world.start_child().await;
    assert_eq!(started.child_id.as_str().len(), 36);

    // The initial prompt was accepted exactly once, with the returned
    // message id, and nothing else reached the child yet.
    let child = world.factory.agent(&started.child_id);
    let followups = child.followups.lock().clone();
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].id, started.message_id);
    assert_eq!(texts(&followups), vec!["child task".to_string()]);

    // The pre-turn descriptor was seeded model-hidden into the child log.
    let events = child.session.events();
    let descriptor = events
        .iter()
        .find(|event| event.type_ == "subagent/descriptor")
        .expect("descriptor");
    assert_eq!(descriptor.data["mode"], "continuable");
    assert_eq!(descriptor.data["provider"], "spawn");
    assert_eq!(descriptor.data["label"], "child task");
    assert!(descriptor.surface_op.is_none());

    // Durable child metadata: parent linkage, origin, and depth.
    let header = child.session.header();
    assert_eq!(header.parent_session.as_ref(), Some(world.parent.id()));
    assert_eq!(header.origin.as_deref(), Some("subagent"));
    assert_eq!(header.delegation_depth, Some(1));

    // The provider contributed its detached creation inputs exactly once,
    // and the parent received nothing for the child's acceptance.
    assert_eq!(world.host.prepares.lock().len(), 1);
    assert!(world.parent.followups.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_requires_persistence() {
    let world = setup_with(false).await;
    let error = world
        .manager
        .start_continuable(start_spec(world.parent_agent()))
        .await
        .expect_err("persistence");
    assert_eq!(error.code, "PERSISTENCE_UNAVAILABLE");
    assert!(world.factory.created.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_propagates_a_non_continuable_provider_rejection() {
    let world = setup().await;
    world.host.prepare_error.lock().replace(SubagentError::new(
        "SUBAGENT_NOT_CONTINUABLE",
        "provider \"spawn\" does not support continuable children",
    ));
    let error = world
        .manager
        .start_continuable(start_spec(world.parent_agent()))
        .await
        .expect_err("capability");
    assert_eq!(error.code, "SUBAGENT_NOT_CONTINUABLE");
    assert!(world.factory.created.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_rolls_back_when_the_signal_aborts_during_preparation() {
    let world = setup().await;
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    world.host.abort_on_prepare.lock().replace(abort.clone());
    let mut spec = start_spec(world.parent_agent());
    let signal: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || abort.load(std::sync::atomic::Ordering::SeqCst));
    spec.signal = signal.clone();
    spec.request.signal = signal;

    let error = world
        .manager
        .start_continuable(spec)
        .await
        .expect_err("aborted");
    assert_eq!(error.code, "CANCELLED");
    // Preparation ran, but no child was materialized.
    assert_eq!(world.host.prepares.lock().len(), 1);
    assert!(world.factory.created.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_rejects_an_excessive_depth() {
    let world = setup().await;
    let mut spec = start_spec(world.parent_agent());
    spec.request.max_depth = Some(0);
    let error = world
        .manager
        .start_continuable(spec)
        .await
        .expect_err("depth");
    assert_eq!(error.code, "DEPTH_EXCEEDED");
    assert!(world.factory.created.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_records_declared_composition_in_the_descriptor() {
    let world = setup().await;
    let mut spec = start_spec(world.parent_agent());
    spec.request.persona = Some("You are scoped.".to_string());
    spec.request.tool_filter = Some(dsh_tools::ToolRestriction {
        allow: None,
        deny: Some(vec!["noop".to_string()]),
    });
    let started = world.manager.start_continuable(spec).await.expect("start");

    let child = world.factory.agent(&started.child_id);
    let events = child.session.events();
    let descriptor = events
        .iter()
        .find(|event| event.type_ == "subagent/descriptor")
        .expect("descriptor");
    assert_eq!(descriptor.data["persona"], "You are scoped.");
    assert_eq!(descriptor.data["toolFilter"]["deny"][0], "noop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_continuable_rejects_a_failing_factory_create() {
    let world = setup().await;
    world.factory.error.lock().replace("no loop".to_string());
    let error = world
        .manager
        .start_continuable(start_spec(world.parent_agent()))
        .await
        .expect_err("create");
    assert_eq!(error.code, "CHILD_CREATE_FAILED");
    // No Activation was installed, so a followup reports the child as
    // non-resident.
    let error = world
        .manager
        .followup(
            world.parent_agent(),
            &session_id("whatever"),
            &text("late"),
            &followup_options(),
        )
        .await
        .expect_err("resumable");
    assert_eq!(error.code, "NOT_RESUMABLE");
}

// ---- followup residency routing ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn followup_delivers_to_a_resident_child_in_order() {
    let world = setup().await;
    let started = world.start_child().await;

    let first = world
        .manager
        .followup(
            world.parent_agent(),
            &started.child_id,
            &text("first follow-up"),
            &followup_options(),
        )
        .await
        .expect("first");
    let second = world
        .manager
        .followup(
            world.parent_agent(),
            &started.child_id,
            &text("second follow-up"),
            &followup_options(),
        )
        .await
        .expect("second");
    assert_ne!(first, second);

    let child = world.factory.agent(&started.child_id);
    assert_eq!(
        texts(&child.followups.lock().clone()),
        vec![
            "child task".to_string(),
            "first follow-up".to_string(),
            "second follow-up".to_string(),
        ]
    );
    // The same Activation kept serving: one child Agent only.
    assert_eq!(world.factory.created.lock().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn followup_rejects_an_unknown_child() {
    let world = setup().await;
    let error = world
        .manager
        .followup(
            world.parent_agent(),
            &session_id("missing"),
            &text("hello"),
            &followup_options(),
        )
        .await
        .expect_err("resumable");
    assert_eq!(error.code, "NOT_RESUMABLE");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn followup_rejects_a_parent_that_is_not_the_durable_direct_parent() {
    let world = setup().await;
    let started = world.start_child().await;
    let stranger = world.register_agent(ProbeAgent::top("stranger")).await;

    let error = world
        .manager
        .followup(
            stranger,
            &started.child_id,
            &text("mine now"),
            &followup_options(),
        )
        .await
        .expect_err("unauthorized");
    assert_eq!(error.code, "UNAUTHORIZED");
    assert!(
        error.message.contains("belongs to another parent session"),
        "{}",
        error.message
    );
    // The message was not delivered.
    let child = world.factory.agent(&started.child_id);
    assert_eq!(child.followups.lock().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn followup_rejects_a_parent_that_was_replaced_by_a_same_id_agent() {
    let world = setup().await;
    let started = world.start_child().await;
    // A same-id but distinct Agent is not the exact live parent.
    let impostor = ProbeAgent::top("parent");
    let error = world
        .manager
        .followup(
            impostor,
            &started.child_id,
            &text("cross the replacement"),
            &followup_options(),
        )
        .await
        .expect_err("unauthorized");
    assert_eq!(error.code, "UNAUTHORIZED");
}

// ---- settlement and teardown ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settlement_notifies_the_idle_parent() {
    let world = setup().await;
    let ends: Arc<parking_lot::Mutex<Vec<dsh_subagent::SubagentRunEndInfo>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let ends_for_listener = ends.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let ends = ends_for_listener.clone();
        Box::pin(async move {
            if let Some(info) = args
                .first()
                .and_then(|value| cordis::downcast::<dsh_subagent::SubagentRunEndInfo>(value))
            {
                ends.lock().push(info.clone());
            }
            None
        })
    });
    world
        .ctx
        .on("subagent/end", listener, Default::default())
        .await;

    let started = world.start_child().await;
    let child = world.factory.agent(&started.child_id);
    child.set_status(AgentStatus::Idle);

    world.manager.drain().await.expect("drain");

    // The idle parent received the child's settlement notice.
    let delivered = world.parent.followups.lock().clone();
    assert!(
        delivered
            .iter()
            .any(|message| texts(std::slice::from_ref(message))
                .join(" ")
                .contains("finished and will do no further work")),
        "{:?}",
        texts(&delivered)
    );
    assert_eq!(child.cancels.lock().clone(), vec!["Parent".to_string()]);
    // The terminal lifecycle edge was published.
    let ends = ends.lock().clone();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].stop_reason, SubagentStopReason::Completed);
    // The Activation is gone (admission now rejects first with DRAINING).
    let error = world
        .manager
        .followup(
            world.parent_agent(),
            &started.child_id,
            &text("too late"),
            &followup_options(),
        )
        .await
        .expect_err("closed");
    assert_eq!(error.code, "DRAINING");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_closes_admission_and_removes_every_activation() {
    let world = setup().await;
    let first = world.start_child().await;
    let second = world.start_child().await;

    world.manager.drain().await.expect("drain");

    // Both children are no longer reachable: admission rejects while the
    // manager is draining.
    for child_id in [first.child_id.clone(), second.child_id.clone()] {
        let error = world
            .manager
            .followup(
                world.parent_agent(),
                &child_id,
                &text("too late"),
                &followup_options(),
            )
            .await
            .expect_err("closed");
        assert_eq!(error.code, "DRAINING");
    }
    // New materialization and delivery are rejected while draining.
    let error = world
        .manager
        .start_continuable(start_spec(world.parent_agent()))
        .await
        .expect_err("draining");
    assert_eq!(error.code, "DRAINING");
    let error = world
        .manager
        .followup(
            world.parent_agent(),
            &session_id("other"),
            &text("too late"),
            &followup_options(),
        )
        .await
        .expect_err("draining");
    assert_eq!(error.code, "DRAINING");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_descendants_stops_only_the_selected_forest() {
    let world = setup().await;
    let parent_b = world.register_agent(ProbeAgent::top("parent-b")).await;

    let child_a = world.start_child().await;
    let child_b = world
        .manager
        .start_continuable(start_spec(parent_b.clone()))
        .await
        .expect("child b");
    let child_a_agent: Arc<dyn Agent> = world.factory.agent(&child_a.child_id);
    let grandchild = world
        .manager
        .start_continuable(start_spec(child_a_agent.clone()))
        .await
        .expect("grandchild");

    world
        .manager
        .drain_descendants(&[world.parent_agent()])
        .await
        .expect("drain a");

    // A's whole forest is gone.
    let error = world
        .manager
        .followup(
            world.parent_agent(),
            &child_a.child_id,
            &text("too late"),
            &followup_options(),
        )
        .await
        .expect_err("closed admission");
    assert_eq!(error.code, "DRAINING");
    let error = world
        .manager
        .followup(
            child_a_agent,
            &grandchild.child_id,
            &text("too late"),
            &followup_options(),
        )
        .await
        .expect_err("closed admission");
    assert_eq!(error.code, "DRAINING");
    let child_a_probe = world.factory.agent(&child_a.child_id);
    assert!(!child_a_probe.cancels.lock().is_empty());

    // B's child is untouched and can keep accepting work.
    let child_b_probe = world.factory.agent(&child_b.child_id);
    assert!(child_b_probe.cancels.lock().is_empty());
    world
        .manager
        .followup(
            parent_b,
            &child_b.child_id,
            &text("still live"),
            &followup_options(),
        )
        .await
        .expect("still live");
}

// ---- interrupt authority ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_enforces_user_and_ancestor_authority() {
    let world = setup().await;
    let started = world.start_child().await;
    let child_agent: Arc<dyn Agent> = world.factory.agent(&started.child_id);

    // The durable direct parent may interrupt as a user.
    world
        .manager
        .interrupt(
            &started.child_id,
            &SubagentInterruptAuthority::User {
                parent_session_id: world.parent.id().clone(),
            },
        )
        .expect("interrupt");
    assert_eq!(
        world
            .factory
            .agent(&started.child_id)
            .cancels
            .lock()
            .clone(),
        vec!["User".to_string()]
    );

    // A stranger may not.
    let error = world
        .manager
        .interrupt(
            &started.child_id,
            &SubagentInterruptAuthority::User {
                parent_session_id: session_id("stranger"),
            },
        )
        .expect_err("unauthorized");
    assert_eq!(error.code, "UNAUTHORIZED");

    // An ancestor may interrupt a live descendant, a non-ancestor may not.
    let grandchild = world
        .manager
        .start_continuable(start_spec(child_agent.clone()))
        .await
        .expect("grandchild");
    world
        .manager
        .interrupt(
            &grandchild.child_id,
            &SubagentInterruptAuthority::Ancestor {
                agent: world.parent_agent(),
            },
        )
        .expect("ancestor");
    let stranger = world.register_agent(ProbeAgent::top("stranger")).await;
    let error = world
        .manager
        .interrupt(
            &grandchild.child_id,
            &SubagentInterruptAuthority::Ancestor { agent: stranger },
        )
        .expect_err("unauthorized");
    assert_eq!(error.code, "UNAUTHORIZED");

    // An unknown child is a silent no-op.
    world
        .manager
        .interrupt(
            &session_id("missing"),
            &SubagentInterruptAuthority::User {
                parent_session_id: world.parent.id().clone(),
            },
        )
        .expect("noop");
}

// ---- reporting ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_from_delivers_selected_content_to_the_parent() {
    let world = setup().await;
    let started = world.start_child().await;
    let child_agent: Arc<dyn Agent> = world.registry.get(&started.child_id).expect("child");

    let quiet = world
        .manager
        .report_from(
            &child_agent,
            &text("here is my progress"),
            &SubagentReportOptions {
                delivery: SubagentReportDelivery::Quiet,
                signal: never_signal(),
            },
        )
        .await
        .expect("quiet");
    let injections = world.parent.injections.lock().clone();
    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].id, quiet);
    let joined = texts(std::slice::from_ref(&injections[0])).join(" ");
    assert!(joined.contains("reported:"), "{joined}");
    assert!(joined.contains("here is my progress"), "{joined}");
    assert!(world.parent.followups.lock().is_empty());

    let wakeup = world
        .manager
        .report_from(
            &child_agent,
            &text("waking you"),
            &SubagentReportOptions {
                delivery: SubagentReportDelivery::Wakeup,
                signal: never_signal(),
            },
        )
        .await
        .expect("wakeup");
    let followups = world.parent.followups.lock().clone();
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].id, wakeup);
    assert!(
        texts(std::slice::from_ref(&followups[0]))
            .join(" ")
            .contains("waking you")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_from_rejects_an_unregistered_child() {
    let world = setup().await;
    let rogue: Arc<dyn Agent> = ProbeAgent::top("rogue");
    let error = world
        .manager
        .report_from(
            &rogue,
            &text("unsolicited"),
            &SubagentReportOptions {
                delivery: SubagentReportDelivery::Quiet,
                signal: never_signal(),
            },
        )
        .await
        .expect_err("unauthorized");
    assert_eq!(error.code, "UNAUTHORIZED");
}

// ---- runtime-level forwarding ----

/// A provider whose continuable capability is toggled.
struct ProbeProvider {
    name: &'static str,
    continuable: bool,
}

#[async_trait::async_trait]
impl SubagentProvider for ProbeProvider {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    async fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        Err(SubagentError::new(
            "UNUSED",
            "one-shot start is not used in this test",
        ))
    }

    async fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, SubagentError> {
        if self.continuable {
            Ok(ContinuableCreateSpec::default())
        } else {
            Err(SubagentError::new(
                "SUBAGENT_NOT_CONTINUABLE",
                format!(
                    "provider \"{}\" does not support continuable children",
                    self.name()
                ),
            ))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_forwards_continuable_operations_through_the_installed_host() {
    let ctx = Context::root();
    let registry = AgentRegistry::install(&ctx);
    let factory = Arc::new(ProbeFactory::default());
    registry.set_factory(factory.clone());
    let erased: Arc<dyn SessionPersistenceApi> = Arc::new(ColdPersistence::default());
    ctx.register_service(erased);
    let runtime = SubagentRuntime::install(&ctx);
    runtime
        .register_provider(
            &ctx,
            Arc::new(ProbeProvider {
                name: "spawn",
                continuable: true,
            }),
        )
        .expect("provider");
    let parent = ProbeAgent::top("parent");
    let parent_agent: Arc<dyn Agent> = parent.clone();
    registry
        .enter(parent_agent.clone(), None)
        .expect("enter parent");
    registry
        .announce(&parent_agent)
        .await
        .expect("announce parent");

    let started = runtime
        .start_continuable(start_spec(parent.clone()))
        .await
        .expect("start");
    let child = factory.agent(&started.child_id);
    assert_eq!(
        texts(&child.followups.lock().clone()),
        vec!["child task".to_string()]
    );

    // A provider without the continuable capability is rejected by the
    // runtime host seam.
    runtime
        .register_provider(
            &ctx,
            Arc::new(ProbeProvider {
                name: "one-shot",
                continuable: false,
            }),
        )
        .expect("provider");
    let error = runtime
        .start_continuable(ContinuableStartSpec {
            provider: "one-shot".to_string(),
            ..start_spec(parent.clone())
        })
        .await
        .expect_err("capability");
    assert_eq!(error.code, "SUBAGENT_NOT_CONTINUABLE");

    // Scoped teardown flows through the runtime to the manager.
    let parent_agent: Arc<dyn Agent> = parent.clone();
    runtime
        .drain_continuable_descendants(std::slice::from_ref(&parent_agent))
        .await
        .expect("drain");
    let error = runtime
        .followup(
            parent,
            &started.child_id,
            &text("too late"),
            SubagentFollowupOptions {
                source: MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
                signal: never_signal(),
            },
        )
        .await
        .expect_err("closed admission");
    assert_eq!(error.code, "DRAINING");
}
