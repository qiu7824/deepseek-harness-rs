//! Rust port of the TS `service.spec.ts` suite for `dsh-jobs`: a concrete
//! registry registers as `ctx.jobs` and serves the abstract API; a second
//! implementation fails loud.
//!
//! # Deviations
//!
//! - The abstract-seam load fence (the TS `new.target` check) is a
//!   compile-time fact in Rust: the trait has no runtime instance, so a
//!   composition row naming this package cannot register an empty
//!   `ctx.jobs`.
//! - The duplicate-registration panic is contained by the fiber load chain,
//!   so `settle()` reports the generic `plugin callback panicked` error.

use std::sync::Arc;

use cordis::{Context, Plugin};
use parking_lot::Mutex;

use dsh_agent::Agent;
use dsh_jobs::{
    JobDoneListener, JobHooks, JobId, JobOutcome, JobOutcomeStatus, JobRead, JobRegistry,
    JobSnapshot, JobStart, JobStatus, JobsChangedListener, KillOutcome, job_id,
};

/// The TS `StubJobRegistry`: one canned record.
struct StubJobRegistry;

impl StubJobRegistry {
    fn snapshot_of(&self, id: &JobId) -> JobSnapshot {
        JobSnapshot {
            id: id.clone(),
            kind: "bash".to_string(),
            label: "sleep 60".to_string(),
            output_limit_bytes: None,
            owner_session: None,
            status: JobStatus::Running,
            detail: None,
            started_at: 0,
            finished_at: None,
            reported: false,
        }
    }
}

impl JobRegistry for StubJobRegistry {
    fn start(&self, spec: JobStart) -> Result<JobId, String> {
        let _hooks = (spec.run)();
        Ok(job_id(format!("{}-1", spec.kind)))
    }

    fn list(&self, _caller: Option<&Arc<dyn Agent>>) -> Vec<JobSnapshot> {
        vec![self.snapshot_of(&job_id("bash-1"))]
    }

    fn get(&self, id: &JobId, _caller: Option<&Arc<dyn Agent>>) -> Result<JobSnapshot, String> {
        Ok(self.snapshot_of(id))
    }

    fn read(&self, id: &JobId, _caller: Option<&Arc<dyn Agent>>) -> Result<JobRead, String> {
        Ok(JobRead {
            text: String::new(),
            snapshot: self.snapshot_of(id),
        })
    }

    fn kill(
        &self,
        _id: &JobId,
        _caller: Option<&Arc<dyn Agent>>,
        _reason: Option<String>,
    ) -> Result<KillOutcome, String> {
        Ok(KillOutcome::Requested)
    }

    fn wait(
        &self,
        id: &JobId,
        _timeout_ms: u64,
        _caller: Option<&Arc<dyn Agent>>,
        _signal: Option<dsh_jobs::JobAbort>,
    ) -> futures::future::BoxFuture<'static, Result<JobSnapshot, String>> {
        let snapshot = self.snapshot_of(id);
        Box::pin(async move { Ok(snapshot) })
    }

    fn on_job_done(&self, _caller: &Context, _listener: JobDoneListener) -> cordis::Disposer {
        cordis::make_disposer(|| Box::pin(async {}))
    }

    fn on_jobs_changed(
        &self,
        _caller: &Context,
        _listener: JobsChangedListener,
    ) -> cordis::Disposer {
        cordis::make_disposer(|| Box::pin(async {}))
    }

    fn attach_controller(&self, _caller: &Context, _name: &str) -> cordis::Disposer {
        cordis::make_disposer(|| Box::pin(async {}))
    }
}

/// The TS hand-built pending hooks (a never-settling job).
struct PendingHooks;

impl JobHooks for PendingHooks {
    fn cancel(&self, _reason: Option<String>) {}

    fn done(&self) -> futures::future::BoxFuture<'static, JobOutcome> {
        Box::pin(std::future::pending())
    }

    fn read_output(&self) -> Option<String> {
        None
    }
}

/// The plugin form (the TS `ctx.plugin(StubJobRegistry)`).
struct StubJobRegistryPlugin;

#[async_trait::async_trait]
impl Plugin for StubJobRegistryPlugin {
    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let erased: Arc<dyn JobRegistry> = Arc::new(StubJobRegistry);
        ctx.register_service(erased);
        Ok(())
    }
}

async fn boot() -> (Context, Arc<dyn JobRegistry>) {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(StubJobRegistryPlugin), cordis::arc(()));
    fiber.settle().await.expect("registry loads");
    let registry = ctx
        .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
        .map(|slot| slot.as_ref().clone())
        .expect("jobs service registered");
    (ctx, registry)
}

#[tokio::test(flavor = "current_thread")]
async fn a_concrete_subclass_registers_as_ctx_jobs_and_serves_the_abstract_api() {
    let (ctx, registry) = boot().await;

    let detach_controller = registry.attach_controller(&ctx, "seam-test");
    let id = registry
        .start(JobStart {
            kind: "bash".to_string(),
            label: "sleep 60".to_string(),
            output_limit_bytes: None,
            owner: None,
            run: Arc::new(|| Arc::new(PendingHooks)),
        })
        .expect("start");
    assert_eq!(id.as_str(), "bash-1");
    assert_eq!(registry.list(None).len(), 1);
    assert_eq!(
        registry.get(&id, None).expect("get").status,
        JobStatus::Running
    );
    assert_eq!(registry.read(&id, None).expect("read").text, "");
    assert_eq!(
        registry.kill(&id, None, None).expect("kill"),
        KillOutcome::Requested
    );
    let waited = registry.wait(&id, 5, None, None).await.expect("wait");
    assert_eq!(waited.id, id);
    let detach_listener = registry.on_job_done(&ctx, Arc::new(|_snapshot, _owner| {}));
    (detach_listener)().await;
    let detach_changes = registry.on_jobs_changed(&ctx, Arc::new(|_owner| {}));
    (detach_changes)().await;
    (detach_controller)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn loading_a_second_implementation_fails() {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(StubJobRegistryPlugin), cordis::arc(()));
    fiber.settle().await.expect("first registry loads");
    // One jobs service per context — the second registration panics inside
    // `apply` and fails the fiber (cordis standard duplicate-service
    // behavior).
    let second = ctx.plugin(Arc::new(StubJobRegistryPlugin), cordis::arc(()));
    let error = second.settle().await.err().expect("second load fails");
    assert!(error.message().contains("panicked"), "{}", error.message());
}
