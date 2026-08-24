//! Topology integration tests: Rust port of
//! `packages/llm/llm/tests/topology.spec.ts` (`llm/adapters-updated`,
//! configurable-provider directory, model discovery registry).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use dsh_llm::*;

type DiscoveryFn = Arc<
    dyn Fn(
            &LlmModelDiscoveryRequest,
        ) -> cordis::BoxFuture<'static, Result<Vec<LlmDiscoveredModel>, String>>
        + Send
        + Sync,
>;

struct NoopAdapter;

impl LlmAdapter for NoopAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        panic!("not exercised");
    }
}

fn entry() -> LlmConfigurableProvider {
    LlmConfigurableProvider {
        provider: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        settings_ns: "llm-pi-ai".to_string(),
        settings_path: vec!["providers".to_string(), "openai".to_string()],
        declared: None,
    }
}

#[tokio::test]
async fn adapters_updated_fires_at_both_commit_points_with_readable_registry() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let observed: Arc<parking_lot::Mutex<Vec<Vec<String>>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let listener_runtime = Arc::clone(&runtime);
    let listener_observed = Arc::clone(&observed);
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let runtime = Arc::clone(&listener_runtime);
        let observed = Arc::clone(&listener_observed);
        Box::pin(async move {
            observed.lock().push(
                runtime
                    .list_providers()
                    .iter()
                    .map(|p| p.id.clone())
                    .collect(),
            );
            None
        })
    });
    ctx.on(
        "llm/adapters-updated",
        listener,
        cordis::EventOptions::default(),
    )
    .await;

    let dispose = runtime
        .register_adapter(
            &ctx,
            vec!["a".to_string(), "b".to_string()],
            Arc::new(NoopAdapter),
        )
        .expect("register");
    assert_eq!(
        *observed.lock(),
        vec![vec!["a".to_string(), "b".to_string()]]
    );
    (dispose.dispose)().await;
    assert_eq!(
        *observed.lock(),
        vec![vec!["a".to_string(), "b".to_string()], Vec::<String>::new()]
    );
}

#[tokio::test]
async fn contains_throwing_listener_without_vetoing_registration() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let later = Arc::new(AtomicU32::new(0));
    let throwing: Arc<cordis::Listener> =
        Arc::new(|_ctx, _args| Box::pin(async { panic!("broken observer") }));
    let later_runs = Arc::clone(&later);
    let observing: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let later = Arc::clone(&later_runs);
        Box::pin(async move {
            later.fetch_add(1, Ordering::SeqCst);
            None
        })
    });
    ctx.on(
        "llm/adapters-updated",
        throwing,
        cordis::EventOptions::default(),
    )
    .await;
    ctx.on(
        "llm/adapters-updated",
        observing,
        cordis::EventOptions::default(),
    )
    .await;

    runtime
        .register_adapter(&ctx, vec!["a".to_string()], Arc::new(NoopAdapter))
        .expect("register");
    assert_eq!(
        runtime.list_providers(),
        vec![LlmProviderInfo {
            id: "a".to_string(),
            name: "a".to_string()
        }]
    );
    assert_eq!(later.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rethrows_first_invariant_listener_failure_after_notifying_rest() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let later = Arc::new(AtomicU32::new(0));
    let failing: Arc<cordis::Listener> = Arc::new(|_ctx, _args| {
        Box::pin(async {
            panic!(
                "{}",
                dsh_invariants::InvariantError::new("@deepseek-ai/test", "registry incoherent")
            );
        })
    });
    let later_runs = Arc::clone(&later);
    let observing: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let later = Arc::clone(&later_runs);
        Box::pin(async move {
            later.fetch_add(1, Ordering::SeqCst);
            None
        })
    });
    ctx.on(
        "llm/adapters-updated",
        failing,
        cordis::EventOptions::default(),
    )
    .await;
    ctx.on(
        "llm/adapters-updated",
        observing,
        cordis::EventOptions::default(),
    )
    .await;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime
            .register_adapter(&ctx, vec!["a".to_string()], Arc::new(NoopAdapter))
            .expect("register");
    }));
    let payload = outcome.expect_err("invariant failure must rethrow");
    let rendered = match payload.downcast_ref::<String>() {
        Some(message) => message.clone(),
        _ => panic!("expected a string panic payload"),
    };
    assert!(rendered.contains("registry incoherent"), "got {rendered}");
    assert_eq!(later.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn directory_registers_lists_and_disposes() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let events = Arc::new(AtomicU32::new(0));
    let events_listener = Arc::clone(&events);
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let events = Arc::clone(&events_listener);
        Box::pin(async move {
            events.fetch_add(1, Ordering::SeqCst);
            None
        })
    });
    ctx.on(
        "llm/adapters-updated",
        listener,
        cordis::EventOptions::default(),
    )
    .await;

    let handle = runtime
        .register_configurable_providers(&ctx, vec![entry()])
        .expect("register");
    assert_eq!(events.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.list_configurable_providers(), vec![entry()]);
    (handle.dispose)().await;
    assert!(runtime.list_configurable_providers().is_empty());
    assert_eq!(events.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn directory_rejects_invalid_entries_all_or_nothing() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);

    let dormant = runtime
        .register_configurable_providers(&ctx, Vec::new())
        .expect("empty directory is a live dormant registration");
    (dormant.replace)(vec![entry()]).expect("activate dormant directory");
    assert_eq!(runtime.list_configurable_providers().len(), 1);
    (dormant.replace)(Vec::new()).expect("return directory to dormant state");

    for invalid in [
        LlmConfigurableProvider {
            provider: String::new(),
            ..entry()
        },
        LlmConfigurableProvider {
            display_name: String::new(),
            ..entry()
        },
        LlmConfigurableProvider {
            settings_ns: String::new(),
            ..entry()
        },
        LlmConfigurableProvider {
            settings_path: vec!["providers".to_string(), String::new()],
            ..entry()
        },
    ] {
        let mut valid_first = entry();
        valid_first.provider = "valid-first".to_string();
        let error = runtime
            .register_configurable_providers(&ctx, vec![valid_first, invalid])
            .err()
            .expect("invalid entry must reject");
        assert_eq!(error.code, "INVALID_DIRECTORY");
        assert!(runtime.list_configurable_providers().is_empty());
    }
}

#[tokio::test]
async fn directory_replaces_atomically_and_guards_disposed_handle() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let mut second = entry();
    second.provider = "second".to_string();
    let handle = runtime
        .register_configurable_providers(&ctx, vec![entry(), second])
        .expect("register");
    let mut elsewhere = entry();
    elsewhere.provider = "owned-elsewhere".to_string();
    runtime
        .register_configurable_providers(&ctx, vec![elsewhere])
        .expect("register");

    // A candidate another registration already declares refuses the whole swap.
    let mut colliding = entry();
    colliding.provider = "owned-elsewhere".to_string();
    let error = (handle.replace)(vec![colliding]).expect_err("collision must refuse");
    assert_eq!(error.code, "DUPLICATE_DIRECTORY");
    let providers: Vec<String> = runtime
        .list_configurable_providers()
        .iter()
        .map(|view| view.provider.clone())
        .collect();
    assert!(providers.contains(&"owned-elsewhere".to_string()));
    assert!(providers.contains(&"second".to_string()));
    assert!(providers.contains(&"openai".to_string()));

    // Its own entries are not "already declared" against itself.
    let mut renamed = entry();
    renamed.display_name = "Renamed".to_string();
    (handle.replace)(vec![renamed]).expect("replace");
    let openai = runtime
        .list_configurable_providers()
        .iter()
        .find(|view| view.provider == "openai")
        .expect("openai retained")
        .clone();
    assert_eq!(openai.display_name, "Renamed");

    // An empty replacement is legal, unlike an empty initial registration.
    (handle.replace)(Vec::new()).expect("empty replace");
    let providers: Vec<String> = runtime
        .list_configurable_providers()
        .iter()
        .map(|view| view.provider.clone())
        .collect();
    assert_eq!(providers, vec!["owned-elsewhere".to_string()]);

    (handle.dispose)().await;
    let error = (handle.replace)(vec![entry()]).expect_err("disposed replace must refuse");
    assert_eq!(error.code, "REGISTRATION_DISPOSED");
}

struct DirectoryPlugin;

#[async_trait::async_trait]
impl Plugin for DirectoryPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("test-directory")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["llm"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let llm = ctx
            .get_typed::<Arc<LlmRuntime>>("llm", false)
            .expect("llm service");
        llm.register_configurable_providers(ctx, vec![entry()])
            .expect("register");
        Ok(())
    }
}

#[tokio::test]
async fn directory_withdraws_entries_when_contributing_fiber_disposes() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let fiber = ctx.plugin(Arc::new(DirectoryPlugin), arc(()));
    fiber.settle().await.expect("fiber settles");
    assert_eq!(runtime.list_configurable_providers().len(), 1);
    fiber.dispose().await;
    assert!(runtime.list_configurable_providers().is_empty());
}

#[tokio::test]
async fn discovery_registers_serves_dedupes_and_disposes() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let seen_request: Arc<parking_lot::Mutex<Option<LlmModelDiscoveryRequest>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let seen_for_closure = Arc::clone(&seen_request);
    let discover: DiscoveryFn = Arc::new(move |request: &LlmModelDiscoveryRequest| {
        let seen = Arc::clone(&seen_for_closure);
        let request = request.clone();
        Box::pin(async move {
            *seen.lock() = Some(request);
            Ok(vec![
                LlmDiscoveredModel {
                    id: "keep".to_string(),
                    name: Some("Keep".to_string()),
                    context_window: Some(1024),
                    max_tokens: Some(256),
                },
                LlmDiscoveredModel {
                    id: String::new(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                },
                LlmDiscoveredModel {
                    id: "keep".to_string(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                },
                LlmDiscoveredModel {
                    id: "bare".to_string(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                },
            ])
        })
    });

    let dispose = runtime
        .register_model_discovery(&ctx, "llm-example", discover)
        .expect("register");
    let request = LlmModelDiscoveryRequest {
        base_url: Some("https://gateway.example/v1".to_string()),
        ..LlmModelDiscoveryRequest::default()
    };
    let models = runtime
        .discover_models("llm-example", &request)
        .await
        .expect("discover");
    assert_eq!(
        models,
        vec![
            LlmDiscoveredModel {
                id: "keep".to_string(),
                name: Some("Keep".to_string()),
                context_window: Some(1024),
                max_tokens: Some(256),
            },
            LlmDiscoveredModel {
                id: "bare".to_string(),
                name: None,
                context_window: None,
                max_tokens: None,
            },
        ]
    );
    assert_eq!(
        seen_request.lock().as_ref().expect("seen").base_url,
        request.base_url
    );

    dispose().await;
    let error = runtime
        .discover_models("llm-example", &request)
        .await
        .expect_err("disposed discovery must refuse");
    assert_eq!(error.code, "NO_DISCOVERY");
}

#[tokio::test]
async fn discovery_rejects_unnamed_namespace_duplicates_and_endpointless_drafts() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    let discover: DiscoveryFn = Arc::new(|_request| Box::pin(async { Ok(Vec::new()) }));

    let error = match runtime.register_model_discovery(&ctx, "", Arc::clone(&discover)) {
        Err(error) => error,
        Ok(_) => panic!("unnamed namespace must reject"),
    };
    assert_eq!(error.code, "INVALID_DISCOVERY");

    runtime
        .register_model_discovery(&ctx, "llm-example", discover)
        .expect("register");
    let duplicate: DiscoveryFn = Arc::new(|_request| Box::pin(async { Ok(Vec::new()) }));
    let error = match runtime.register_model_discovery(&ctx, "llm-example", duplicate) {
        Err(error) => error,
        Ok(_) => panic!("duplicate namespace must reject"),
    };
    assert_eq!(error.code, "DUPLICATE_DISCOVERY");

    let request = LlmModelDiscoveryRequest {
        base_url: Some("https://gateway.example/v1".to_string()),
        ..LlmModelDiscoveryRequest::default()
    };
    let error = runtime
        .discover_models("llm-absent", &request)
        .await
        .expect_err("absent namespace must refuse");
    assert_eq!(error.code, "NO_DISCOVERY");
    let error = runtime
        .discover_models("llm-example", &LlmModelDiscoveryRequest::default())
        .await
        .expect_err("endpointless draft must refuse");
    assert_eq!(error.code, "INVALID_DISCOVERY");

    // Naming a route alone is enough: the adapter may know it without an
    // endpoint.
    let route_only = LlmModelDiscoveryRequest {
        provider: Some("known-route".to_string()),
        ..LlmModelDiscoveryRequest::default()
    };
    assert!(
        runtime
            .discover_models("llm-example", &route_only)
            .await
            .expect("discover")
            .is_empty()
    );
}
