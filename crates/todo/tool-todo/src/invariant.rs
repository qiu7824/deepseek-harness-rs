//! Package-owned durable todo-snapshot invariants. Rust port of
//! `packages/todo/tool-todo/src/invariant.ts`.
//!
//! Deliberately silent on how many items are `in_progress`: that is the
//! tool's per-deployment policy, not a durable-shape rule (a log written
//! while parallel work was allowed must still replay after a deployment
//! tightens the policy).

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_session::{SessionEvent, SessionStore};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-tool-todo";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "tool-todo-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

const TODO_STATUSES: [&str; 3] = ["pending", "in_progress", "completed"];

/// Validate one whole-list todo snapshot (TS `validateTodos`; failures carry
/// the exact TS messages).
pub fn validate_todos(value: &serde_json::Value) -> Result<(), String> {
    let items = value
        .as_array()
        .ok_or_else(|| "todo/write todos must be an array".to_string())?;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| "todo/write entries must be objects".to_string())?;
        let content = object
            .get("content")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                "todo/write content must be non-empty and already trimmed".to_string()
            })?;
        if content.is_empty() || content.trim() != content {
            return Err("todo/write content must be non-empty and already trimmed".to_string());
        }
        if !seen.insert(content.to_string()) {
            return Err(format!(
                "todo/write repeats content {}",
                serde_json::to_string(content).expect("content")
            ));
        }
        let status = object.get("status").and_then(|value| value.as_str());
        if !status.is_some_and(|status| TODO_STATUSES.contains(&status)) {
            return Err(format!(
                "todo/write carries unknown status {}",
                serde_json::to_string(&status).expect("status")
            ));
        }
    }
    Ok(())
}

/// Validate the package-owned event fields and ignore unrelated events (TS
/// `validateEvent`).
pub fn validate_event(event: &SessionEvent) -> Result<(), String> {
    if event.type_ == "todo/write" {
        validate_todos(
            event
                .data
                .get("todos")
                .unwrap_or(&serde_json::Value::Null),
        )?;
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
                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        for event in session.events().iter() {
                            if let Err(message) = validate_event(event) {
                                fail(&message);
                            }
                        }
                    }
                }

                // Validate each event before publication (reads only the
                // event — no session state, so the append-time lock is safe).
                let fail_for_dispatch = fail.clone();
                let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
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
                        let Some(event) = event_args
                            .get(1)
                            .and_then(|value| downcast::<SessionEvent>(value))
                        else {
                            return None;
                        };
                        if let Err(message) = validate_event(event) {
                            fail(&message);
                        }
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
        .expect("the tool-todo invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct ToolTodoInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for ToolTodoInvariantPlugin {
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
