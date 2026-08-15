//! Package-owned permission-preset event invariants: every durable
//! `permission/preset` payload must name a configured table entry, so
//! replay folds stay resolvable. Rust port of
//! `packages/interaction/permission-presets/src/invariant.ts`.
//!
//! # Deviations
//!
//! - The `internal/dispatch` pre-hook runs inline while `Session::append`
//!   holds the session state lock; reading the service's immutable preset
//!   table needs no lock and never reenters the session, so the companion
//!   needs no per-session trace here.

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{SessionEvent, SessionStore};

use crate::PermissionPresetService;

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-permission-presets";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "permission-presets-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate the package-owned event fields and ignore unrelated events (TS
/// `validateEvent`; failures carry the exact TS messages).
pub fn validate_event(
    service: &PermissionPresetService,
    event: &SessionEvent,
) -> Result<(), String> {
    if event.type_ == "permission/preset" {
        let preset = event
            .data
            .get("preset")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if !service.names().contains(&preset) {
            return Err(format!(
                "permission/preset names unknown preset {}",
                serde_json::to_string(preset).expect("preset")
            ));
        }
    }
    Ok(())
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["permissionPresets", "sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let service = ctx
                    .get_typed::<Arc<PermissionPresetService>>("permissionPresets", false)
                    .map(|slot| slot.as_ref().clone());

                // Seed every attached session.
                if let (Some(service), Some(store)) = (
                    service.clone(),
                    ctx.get_typed::<Arc<SessionStore>>("sessions", false)
                        .map(|slot| slot.as_ref().clone()),
                ) {
                    for session in store.list() {
                        for event in session.events().iter() {
                            if let Err(message) = validate_event(&service, event) {
                                fail(&message);
                            }
                        }
                    }
                }

                // Validate each preset event before publication.
                let service_for_dispatch = service;
                let fail_for_dispatch = fail.clone();
                let dispatch_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let event_name = args
                        .get(1)
                        .and_then(|value| downcast::<String>(value))
                        .cloned()
                        .unwrap_or_default();
                    let event_args = args
                        .get(2)
                        .and_then(|value| downcast_arc::<Vec<ArcValue>>(value));
                    let service = service_for_dispatch.clone();
                    let fail = fail_for_dispatch.clone();
                    Box::pin(async move {
                        if event_name != "session/event" {
                            return None;
                        }
                        let Some(event_args) = event_args else {
                            return None;
                        };
                        let event = event_args
                            .get(1)
                            .and_then(|value| downcast::<SessionEvent>(value))
                            .cloned();
                        let (Some(service), Some(event)) = (service, event) else {
                            return None;
                        };
                        if let Err(message) = validate_event(&service, &event) {
                            fail(&message);
                        }
                        None
                    })
                });
                ctx.on(
                    "internal/dispatch",
                    dispatch_listener,
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
        .expect("the permission-presets invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct PermissionPresetsInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for PermissionPresetsInvariantPlugin {
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
