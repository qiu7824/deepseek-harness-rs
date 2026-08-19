//! Package-owned filesystem event-data invariants. Rust port of
//! `packages/fs/fs/src/invariant.ts`: the `internal/dispatch` pre-hook
//! validates the three filesystem event payloads before any listener runs.

use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, Plugin, PluginError, downcast};

use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::types::{FsObservation, FsTarget};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-fs";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "fs-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Assert that an event carries a usable opaque target identity (the TS
/// `validateTarget`).
pub fn validate_target(target: &FsTarget, fail: &dyn Fn(&str)) {
    if target.target_key.as_str().is_empty() {
        fail("filesystem event targetKey must be non-empty");
    }
    if target.display_path.is_empty() {
        fail("filesystem event displayPath must be non-empty");
    }
}

/// The pure check over one dispatch (exported for the unit spec).
pub fn check_dispatch(event_name: &str, args: &[ArcValue], fail: &dyn Fn(&str)) {
    if event_name != "fs/write-intent"
        && event_name != "fs/edit-intent"
        && event_name != "fs/observed"
    {
        return;
    }
    let Some(target) = args.first().and_then(|value| downcast::<FsTarget>(value)) else {
        return;
    };
    validate_target(target, fail);
    if event_name == "fs/observed" {
        let Some(observation) = args
            .get(1)
            .and_then(|value| downcast::<FsObservation>(value))
        else {
            return;
        };
        match observation {
            FsObservation::Present { version } => {
                if version.as_str().is_empty() {
                    fail("fs/observed present version must be non-empty");
                }
            }
            FsObservation::Absent => {}
        }
    }
}

/// Build the installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let fail_for_listener = fail.clone();
                let listener: Arc<cordis::Listener> =
                    Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                        // internal/dispatch args: [mode, eventName, eventArgs, ctx]
                        let event_name = args
                            .get(1)
                            .and_then(|value| downcast::<String>(value))
                            .cloned();
                        let event_args = args
                            .get(2)
                            .and_then(|value| downcast::<Vec<ArcValue>>(value))
                            .cloned();
                        let fail = fail_for_listener.clone();
                        Box::pin(async move {
                            let (Some(event_name), Some(event_args)) = (event_name, event_args)
                            else {
                                return None;
                            };
                            check_dispatch(&event_name, &event_args, &|message| fail(message));
                            None
                        })
                    });
                ctx.on(
                    "internal/dispatch",
                    listener,
                    EventOptions::default().global(true),
                )
                .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the fs invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct FsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for FsInvariantPlugin {
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
