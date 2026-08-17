//! Invariant companion integration tests: Rust port of the core subset of
//! `tests/invariant.spec.ts`.
//!
//! Covers: a service published into the root realm AFTER the mount audit
//! (from an async continuation) is caught by the package invariant, and
//! the unjoined-agent check staying inert when no agent assembles.

mod common;

use std::sync::Arc;

use common::{boot, scoped, seed_preset, temp_dir};
use cordis::{ArcValue, Context, Plugin, PluginError};
use parking_lot::Mutex;

/// The TS `late-service` fixture: publishes into the ROOT realm only after
/// its plugin body returned, escaping the one-shot mount audit.
struct LateServicePlugin {
    publish_slot: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

#[async_trait::async_trait]
impl Plugin for LateServicePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("late-service")
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = cordis::downcast::<serde_json::Value>(&config)
            .cloned()
            .unwrap_or_default();
        let service = config
            .get("service")
            .and_then(|value| value.as_str())
            .unwrap_or("fixtureLateSvc")
            .to_string();
        let ctx = ctx.clone();
        let publish: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let ctx = ctx.clone();
            let service = service.clone();
            tokio::spawn(async move {
                ctx.reflect
                    .provide(&ctx, &service, Some(cordis::arc("late".to_string())), None);
            });
        });
        *self.publish_slot.lock() = Some(publish);
        Ok(())
    }
}

const LATE: &str = "- id: late\n  name: late-service\n  config:\n    service: fixtureLateSvc\n";

#[tokio::test]
async fn catches_a_service_published_after_the_mount_audit() {
    let ctx = boot().await;
    let publish_slot: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>> = Arc::new(Mutex::new(None));
    let loader = ctx
        .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", true)
        .map(|double_arc| (*double_arc).clone())
        .expect("loader service");
    loader.core.register(
        "late-service",
        Arc::new(LateServicePlugin {
            publish_slot: publish_slot.clone(),
        }),
    );

    // Run the installer with a recording failure channel (the registry's
    // child-fiber wrapping is not needed for the listener wiring).
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new({
        let failures = failures.clone();
        move |message: &str| failures.lock().push(message.to_string())
    });
    let installer = dsh_agent_presets::invariant::installer();
    (installer.install)(&ctx, fail).await;

    // Mount the late preset: the one-shot audit passes (nothing leaked yet).
    let root = temp_dir("invariant-late");
    let preset = seed_preset(&root, "late", LATE).await;
    let (scope, _key) = scoped(&ctx);
    dsh_agent_presets::mount_preset(&scope.ctx, &preset)
        .await
        .expect("mount passes the one-shot audit");

    // The late publication escapes the mount audit and must trip the
    // package invariant's service-listener re-check.
    let publish = publish_slot.lock().take().expect("publish handle stored");
    publish();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let failures = failures.lock();
    assert!(
        failures.iter().any(|message| {
            message.contains("fixtureLateSvc") && message.contains("process-global")
        }),
        "the invariant must catch the late leak, got: {failures:?}"
    );
    drop(failures);
    (scope.dispose)().await;
}

#[tokio::test]
async fn the_unjoined_agent_check_stays_inert_without_an_agent() {
    // The assemble waterfall runs with no agent in the context: the check
    // must not fail (TS no-agent branch).
    let ctx = boot().await;
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new({
        let failures = failures.clone();
        move |message: &str| failures.lock().push(message.to_string())
    });
    let installer = dsh_agent_presets::invariant::installer();
    (installer.install)(&ctx, fail).await;

    // Emit an assemble waterfall with an empty context and no roster: the
    // invariant must stay silent.
    ctx.waterfall(
        "system-prompt/assemble",
        vec![
            cordis::arc(()),
            cordis::arc(dsh_system_prompt::AssembleContext::default()),
        ],
        Box::pin(async move { cordis::arc(()) }),
    )
    .await;

    assert!(
        failures.lock().is_empty(),
        "no agent and no roster must not fail"
    );
}
