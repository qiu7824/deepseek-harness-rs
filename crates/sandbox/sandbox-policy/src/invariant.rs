//! Package-owned session-event invariants for sandbox policy. Rust port of
//! `packages/sandbox/sandbox-policy/src/invariant.ts`: validate the
//! package-owned event fields and ignore unrelated events.

use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, InjectSpec, Plugin, PluginError, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{SessionEvent, SessionStore};

use crate::session_mode::SANDBOX_MODES;

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-sandbox-policy";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "sandbox-policy-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate the package-owned event fields and ignore unrelated events (the
/// TS `validateEvent`).
pub fn validate_event(event: &SessionEvent, fail: &dyn Fn(&str)) {
    if event.type_ != "sandbox/mode" {
        return;
    }
    let Some(mode) = event.data.get("mode").and_then(|mode| mode.as_str()) else {
        fail(&format!(
            "sandbox/mode carries unknown mode {}",
            serde_json::to_string(&event.data).unwrap_or_default()
        ));
        return;
    };
    if !SANDBOX_MODES.iter().any(|known| known.as_str() == mode) {
        fail(&format!("sandbox/mode carries unknown mode {mode:?}"));
    }
}

/// Build the installer (TS `install` + its `sessions` inject).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // Late registration: validate every already-loaded session.
                if let Some(store) = ctx
                    .get_typed::<Arc<Arc<SessionStore>>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        for event in session.events().iter() {
                            validate_event(event, &|message| fail(message));
                        }
                    }
                }
                let fail_for_listener = fail.clone();
                let listener: Arc<cordis::Listener> = Arc::new(
                    move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
                        let event = args.get(1).and_then(|value| downcast::<SessionEvent>(value)).cloned();
                        let fail = fail_for_listener.clone();
                        Box::pin(async move {
                            if let Some(event) = event {
                                validate_event(&event, &|message| fail(message));
                            }
                            None
                        })
                    },
                );
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
        .expect("the sandbox-policy invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct SandboxPolicyInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for SandboxPolicyInvariantPlugin {
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
