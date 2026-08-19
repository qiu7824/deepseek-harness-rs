//! Package-owned background-job snapshot invariants. Rust port of
//! `packages/jobs/jobs/src/invariant.ts`.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_agent::Agent;
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::index::JobRegistry;
use crate::types::{JobSnapshot, JobStatus};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-jobs";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "jobs-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate the cross-field relationships in one registry snapshot (TS
/// `validateSnapshot`).
pub fn validate_snapshot(
    snapshot: &JobSnapshot,
    owner: Option<&Arc<dyn Agent>>,
    fail: &dyn Fn(&str),
) {
    let id = snapshot.id.as_str();
    let prefix = format!("{}-", snapshot.kind);
    let ordinal = id
        .strip_prefix(&prefix)
        .and_then(|rest| rest.parse::<u64>().ok());
    if snapshot.kind.is_empty() || ordinal.is_none() || ordinal == Some(0) {
        fail(&format!(
            "job snapshot id {:?} must be {:?} followed by a positive ordinal",
            id, prefix
        ));
    }
    if snapshot.label.is_empty() {
        fail(&format!("job {id:?} label must be non-empty"));
    }
    // startedAt must be a non-negative epoch integer (u64 is integral by
    // construction; the non-negativity is a type fact).
    let terminal = snapshot.status.is_terminal();
    if terminal != snapshot.finished_at.is_some() {
        fail(&format!(
            "job {id:?} finishedAt must be present exactly for a terminal status"
        ));
    }
    if let Some(finished) = snapshot.finished_at {
        if finished < snapshot.started_at {
            fail(&format!(
                "job {id:?} finishedAt must be an epoch integer no earlier than startedAt"
            ));
        }
    }
    let expected_owner = owner.map(|agent| agent.id().clone());
    if snapshot.owner_session != expected_owner {
        fail(&format!(
            "job {id:?} ownerSession does not match its completion owner"
        ));
    }
}

/// Build the installer (TS `install` + its `jobs` inject).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["jobs"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let Some(registry) = ctx
                    .get_typed::<Arc<dyn JobRegistry>>("jobs", false)
                    .map(|slot| slot.as_ref().clone())
                else {
                    return;
                };
                // Validate current unowned records.
                for snapshot in registry.list(None) {
                    validate_snapshot(&snapshot, None, &|message| fail(message));
                }
                // Validate every terminal snapshot at completion.
                let listener: crate::types::JobDoneListener = Arc::new(
                    move |snapshot: JobSnapshot, owner: Option<Arc<dyn Agent>>| {
                        let fail = fail.clone();
                        validate_snapshot(&snapshot, owner.as_ref(), &|message| fail(message));
                    },
                );
                let _disposer = registry.on_job_done(&ctx, listener);
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the jobs invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct JobsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for JobsInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
