//! Package-owned durable plan-mode invariants: every `plan/mode` payload
//! must carry a boolean `active`. Rust port of
//! `packages/plan/plan-mode/src/invariant.ts`.

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{Session, SessionEvent, SessionStore};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-plan-mode";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "plan-mode-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate one `plan/mode` event payload (TS `validateEvent`; failures
/// carry the exact TS messages).
pub fn validate_event(event: &SessionEvent) -> Result<(), String> {
    if event.type_ != "plan/mode" {
        return Ok(());
    }
    if event.data.get("active").and_then(|value| value.as_bool()).is_none() {
        return Err(format!(
            "plan/mode carries invalid active state {}; expected a boolean",
            serde_json::to_string(&event.data.get("active")).expect("active")
        ));
    }
    Ok(())
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let seed = |session: &Session, fail: &Arc<dyn Fn(&str) + Send + Sync>| {
                    for event in session.events().iter() {
                        if let Err(message) = validate_event(event) {
                            fail(&message);
                        }
                    }
                };
                if let Some(store) = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        seed(&session, &fail);
                    }
                }

                let fail_for_created = fail.clone();
                let created_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let fail = fail_for_created.clone();
                    Box::pin(async move {
                        let session = args
                            .first()
                            .and_then(|value| downcast::<Session>(value))
                            .cloned();
                        if let Some(session) = session {
                            for event in session.events().iter() {
                                if let Err(message) = validate_event(event) {
                                    fail(&message);
                                }
                            }
                        }
                        None
                    })
                });
                ctx.on(
                    "session/created",
                    created_listener,
                    EventOptions::default().global(true),
                )
                .await;

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
                        let Some(event) = event else {
                            return None;
                        };
                        if let Err(message) = validate_event(&event) {
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
        .expect("the plan-mode invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct PlanModeInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for PlanModeInvariantPlugin {
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
