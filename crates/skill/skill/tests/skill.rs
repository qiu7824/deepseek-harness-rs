#![allow(clippy::type_complexity)]
// Test invalidation cells mirror the public callback ownership shape.

//! Rust port of the core `skill.spec.ts` behaviors: provider registration
//! and disposal, rank/order dedup, invocation policies, runtime-skill
//! defaults and duplicate handling, candidate/definition validation,
//! lookup-option borrowing, catalog caching and invalidation, abort
//! racing, change notifications, rendering, and scoped layers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cordis::{Context, LoggerLevel, Message, arc, downcast};
use dsh_scope::{CreateScopeOptions, ScopeKey, bind_scope_parent, create_scope};
use dsh_skill::{
    BUNDLED_SKILL_RANK, Config, SKILL_ABORTED_MESSAGE, SkillAbort, SkillCandidate, SkillDefinition,
    SkillInvocationPolicy, SkillLookupOptions, SkillProvider, SkillProviderControl,
    SkillProviderObservation, SkillRegistration, SkillRegistry, SkillResourceBase,
    SkillViewOptions, escape_text, is_model_invocable, is_skill_name, is_user_invocable,
    render_skill_content,
};

#[derive(Debug, Clone, PartialEq)]
struct Locator {
    content: String,
}

fn memory_skill(name: &str, description: &str, rank: i64, body: Option<&str>) -> SkillCandidate {
    SkillCandidate {
        name: name.to_string(),
        description: description.to_string(),
        when_to_use: None,
        invocation: SkillInvocationPolicy::BOTH,
        provider: "memory".to_string(),
        source: "memory".to_string(),
        resource_base: None,
        rank,
        locator: arc(Locator {
            content: body.unwrap_or(&format!("{name} body.")).to_string(),
        }),
        path: None,
        metadata: None,
    }
}

struct MemoryProvider {
    name: &'static str,
    candidates: parking_lot::Mutex<Vec<SkillCandidate>>,
    list_calls: AtomicU64,
}

impl MemoryProvider {
    fn new(name: &'static str, candidates: Vec<SkillCandidate>) -> Arc<Self> {
        Arc::new(Self {
            name,
            candidates: parking_lot::Mutex::new(candidates),
            list_calls: AtomicU64::new(0),
        })
    }

    fn replace(&self, candidates: Vec<SkillCandidate>) {
        *self.candidates.lock() = candidates;
    }
}

#[async_trait::async_trait]
impl SkillProvider for MemoryProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn list(
        &self,
        _options: &SkillLookupOptions,
    ) -> Result<SkillProviderObservation, String> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        Ok(SkillProviderObservation {
            candidates: self.candidates.lock().clone(),
            complete: true,
        })
    }

    async fn get(
        &self,
        candidate: &SkillCandidate,
        _options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        let locator = downcast::<Locator>(&candidate.locator).expect("locator");
        Ok(Some(SkillDefinition {
            name: candidate.name.clone(),
            description: candidate.description.clone(),
            when_to_use: candidate.when_to_use.clone(),
            invocation: candidate.invocation,
            source: candidate.source.clone(),
            provider: candidate.provider.clone(),
            resource_base: candidate.resource_base.clone(),
            content: locator.content.clone(),
            path: candidate.path.clone(),
            metadata: candidate.metadata.clone(),
        }))
    }
}

fn register_provider(ctx: &Context, provider: Arc<dyn SkillProvider>) -> cordis::Disposer {
    let registry = skills_of(ctx);
    registry.register_provider(ctx, Arc::new(move |_control| provider.clone()))
}

fn skills_of(ctx: &Context) -> Arc<SkillRegistry> {
    ctx.get_typed::<Arc<SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .expect("skills service")
}

fn installed(config: Option<Config>) -> (Context, Arc<SkillRegistry>) {
    let ctx = Context::root();
    let registry = SkillRegistry::install(&ctx, config.unwrap_or_default()).expect("install");
    (ctx, registry)
}

fn runtime_registration(name: &str, description: &str) -> SkillRegistration {
    SkillRegistration {
        name: name.to_string(),
        description: description.to_string(),
        when_to_use: None,
        source: "runtime".to_string(),
        resource_base: None,
        content: format!("{name} body."),
        path: None,
        metadata: None,
        invocation: None,
        provider: None,
    }
}

/// Capture warn messages through the logger service.
struct CaptureExporter {
    warns: parking_lot::Mutex<Vec<String>>,
}

impl cordis::Exporter for CaptureExporter {
    fn default_level(&self) -> LoggerLevel {
        LoggerLevel::Warn
    }

    fn levels(&self) -> &HashMap<String, LoggerLevel> {
        static EMPTY: std::sync::LazyLock<HashMap<String, LoggerLevel>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }

    fn export(&self, message: &Message) {
        if message.level == LoggerLevel::Warn {
            for arg in &message.args {
                if let Some(text) = downcast::<String>(arg) {
                    self.warns.lock().push(text.clone());
                }
            }
        }
    }
}

fn capture_warns(ctx: &Context) -> Arc<CaptureExporter> {
    let exporter = Arc::new(CaptureExporter {
        warns: parking_lot::Mutex::new(Vec::new()),
    });
    let logger = ctx
        .get_typed::<Arc<cordis::LoggerService>>("logger", false)
        .map(|slot| slot.as_ref().clone())
        .expect("logger");
    let _ = logger.exporter(ctx, exporter.clone());
    exporter
}

fn never_abort() -> SkillAbort {
    Arc::new(|| false)
}

// ---- registry ----

#[tokio::test(flavor = "current_thread")]
async fn registers_providers_resolves_duplicates_first_wins_and_disposes() {
    let (ctx, registry) = installed(None);
    let provider = MemoryProvider::new(
        "memory",
        vec![
            memory_skill("z-skill", "Z skill", 20, None),
            memory_skill("a-skill", "A skill", 10, None),
            memory_skill("shadowed", "Lower priority", 20, None),
        ],
    );
    struct OverrideProvider;
    #[async_trait::async_trait]
    impl SkillProvider for OverrideProvider {
        fn name(&self) -> &str {
            "override"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "shadowed".to_string(),
                    description: "Higher priority".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "override".to_string(),
                    source: "override".to_string(),
                    resource_base: None,
                    rank: 5,
                    locator: arc(Locator {
                        content: "Override body.".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    let dispose_memory = register_provider(&ctx, provider.clone());
    register_provider(&ctx, Arc::new(OverrideProvider));

    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let shapes: Vec<(String, String, String)> = listed
        .into_iter()
        .map(|skill| (skill.name, skill.description, skill.provider))
        .collect();
    assert_eq!(
        shapes,
        vec![
            (
                "a-skill".to_string(),
                "A skill".to_string(),
                "memory".to_string()
            ),
            (
                "shadowed".to_string(),
                "Higher priority".to_string(),
                "override".to_string()
            ),
            (
                "z-skill".to_string(),
                "Z skill".to_string(),
                "memory".to_string()
            ),
        ]
    );
    let shadowed = registry
        .get("shadowed", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(shadowed.expect("some").content, "Override body.");

    let same_rank = {
        let mut candidate = memory_skill("same-rank-skill", "Same rank", 10, None);
        candidate.provider = "same-rank".to_string();
        MemoryProvider::new("same-rank", vec![candidate])
    };
    register_provider(&ctx, same_rank.clone());
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(
        listed
            .iter()
            .find(|skill| skill.name == "same-rank-skill")
            .map(|skill| skill.provider.as_str()),
        Some("same-rank")
    );

    // Duplicate provider name within one layer fails loud.
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        register_provider(&ctx, MemoryProvider::new("memory", Vec::new()));
    }));
    assert!(duplicate.is_err(), "duplicate provider names fail loud");

    // The runtime provider name is reserved; the aborted flag records the
    // rejection.
    let rejected_signal = Arc::new(AtomicBool::new(false));
    let rejected_signal_for_factory = rejected_signal.clone();
    let reserved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register_provider(
            &ctx,
            Arc::new(move |control| {
                let _ = control.signal.clone();
                rejected_signal_for_factory.store(true, Ordering::SeqCst);
                MemoryProvider::new("runtime", Vec::new())
            }),
        );
    }));
    assert!(reserved.is_err(), "runtime name is reserved");
    // The factory control signal must be armed by the rejection path.
    let factory_flag = Arc::new(AtomicBool::new(false));
    let factory_failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register_provider(
            &ctx,
            Arc::new(move |control| {
                let signal = control.signal.clone();
                factory_flag.store(signal(), Ordering::SeqCst);
                panic!("factory failed")
            }),
        );
    }));
    assert!(factory_failure.is_err(), "factory failure propagates");

    // Disposing the memory provider drops its skills.
    (dispose_memory)().await;
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["same-rank-skill", "shadowed"]);
}

#[tokio::test(flavor = "current_thread")]
async fn returns_invocation_neutral_catalog_and_resolves_policies_independently() {
    let (ctx, registry) = installed(None);
    let registrations = [
        ("both", None),
        (
            "model-only",
            Some(SkillInvocationPolicy {
                model_invocable: true,
                user_invocable: false,
            }),
        ),
        (
            "user-only",
            Some(SkillInvocationPolicy {
                model_invocable: false,
                user_invocable: true,
            }),
        ),
        (
            "trusted-only",
            Some(SkillInvocationPolicy {
                model_invocable: false,
                user_invocable: false,
            }),
        ),
    ];
    for (name, invocation) in registrations {
        let mut registration = runtime_registration(name, name);
        registration.invocation = invocation;
        let _ = registry.register(&ctx, registration);
    }

    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["both", "model-only", "trusted-only", "user-only"]
    );
    assert_eq!(
        listed
            .iter()
            .find(|skill| skill.name == "both")
            .expect("both")
            .invocation,
        SkillInvocationPolicy::BOTH
    );
    let model: Vec<&str> = listed
        .iter()
        .filter(|skill| is_model_invocable(skill))
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(model, vec!["both", "model-only"]);
    let user: Vec<&str> = listed
        .iter()
        .filter(|skill| is_user_invocable(skill))
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(user, vec!["both", "user-only"]);
    let loaded = registry
        .get("trusted-only", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(loaded.expect("some").content, "trusted-only body.");
    let both = registry
        .get("both", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(both.expect("some").invocation, SkillInvocationPolicy::BOTH);
}

#[tokio::test(flavor = "current_thread")]
async fn validates_candidate_name_description_and_provider_ownership() {
    let (ctx, registry) = installed(None);
    struct BadProvider(&'static str, SkillCandidate);
    #[async_trait::async_trait]
    impl SkillProvider for BadProvider {
        fn name(&self) -> &str {
            self.0
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![self.1.clone()],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }

    let bad_name = memory_skill("Bad_Name", "bad", 1, None);
    register_provider(&ctx, Arc::new(BadProvider("bad-name", bad_name)));
    let error = registry
        .list(SkillViewOptions::default())
        .await
        .expect_err("invalid name");
    assert!(error.contains("invalid skill name"), "{error}");

    let (empty_ctx, empty_registry) = installed(None);
    let mut empty_desc = memory_skill("empty-description", "x", 1, None);
    empty_desc.description = String::new();
    empty_desc.provider = "empty-description".to_string();
    register_provider(
        &empty_ctx,
        Arc::new(BadProvider("empty-description", empty_desc)),
    );
    let error = empty_registry
        .list(SkillViewOptions::default())
        .await
        .expect_err("empty desc");
    assert!(error.contains("without a description"), "{error}");

    let (foreign_ctx, foreign_registry) = installed(None);
    let mut foreign = memory_skill("wrong-provider", "Wrong provider", 1, None);
    foreign.provider = "different".to_string();
    register_provider(
        &foreign_ctx,
        Arc::new(BadProvider("wrong-provider", foreign)),
    );
    let error = foreign_registry
        .list(SkillViewOptions::default())
        .await
        .expect_err("foreign");
    assert!(error.contains("for provider"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn borrows_the_exact_lookup_options_through_discovery_and_loading() {
    let (ctx, registry) = installed(None);
    struct ContextualProvider {
        listed_with: parking_lot::Mutex<Option<(Option<String>, Option<SkillAbort>)>>,
        loaded_with: parking_lot::Mutex<Option<(Option<String>, Option<SkillAbort>)>>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for ContextualProvider {
        fn name(&self) -> &str {
            "contextual"
        }
        async fn list(
            &self,
            options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            *self.listed_with.lock() = Some((options.cwd.clone(), options.signal.clone()));
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "skill-a".to_string(),
                    description: "Skill A".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "contextual".to_string(),
                    source: "test".to_string(),
                    resource_base: None,
                    rank: 1,
                    locator: arc(Locator {
                        content: "Skill A body.".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            *self.loaded_with.lock() = Some((options.cwd.clone(), options.signal.clone()));
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    let signal: SkillAbort = never_abort();
    let provider = Arc::new(ContextualProvider {
        listed_with: parking_lot::Mutex::new(None),
        loaded_with: parking_lot::Mutex::new(None),
    });
    register_provider(&ctx, provider.clone());
    let options = SkillViewOptions {
        cwd: Some("/workspace/a".to_string()),
        signal: Some(signal.clone()),
        scope: None,
    };

    let listed = registry.list(options.clone()).await.expect("list");
    assert_eq!(listed[0].name, "skill-a");
    let loaded = registry.get("skill-a", options.clone()).await.expect("get");
    assert_eq!(loaded.expect("some").content, "Skill A body.");
    let listed_with = provider.listed_with.lock().clone().expect("listed");
    assert_eq!(listed_with.0.as_deref(), Some("/workspace/a"));
    assert!(Arc::ptr_eq(
        listed_with.1.as_ref().expect("signal"),
        &signal
    ));
    let loaded_with = provider.loaded_with.lock().clone().expect("loaded");
    assert_eq!(loaded_with.0.as_deref(), Some("/workspace/a"));
    assert!(Arc::ptr_eq(
        loaded_with.1.as_ref().expect("signal"),
        &signal
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn rechecks_cancellation_after_cached_discovery_before_provider_loading() {
    let (ctx, registry) = installed(None);
    let get_calls = Arc::new(AtomicU64::new(0));
    let get_calls_for_provider = get_calls.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_for_provider = gate.clone();
    struct CachedProvider {
        get_calls: Arc<AtomicU64>,
        gate: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for CachedProvider {
        fn name(&self) -> &str {
            "cached"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "cached-skill".to_string(),
                    description: "Cached skill".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "cached".to_string(),
                    source: "test".to_string(),
                    resource_base: None,
                    rank: 1,
                    locator: arc(Locator {
                        content: "Cached body.".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            // Hold the load so the abort lands at the wait boundary (the
            // Rust recheck merges with the load race — see the deviation
            // note in the registry docs).
            self.gate.notified().await;
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    register_provider(
        &ctx,
        Arc::new(CachedProvider {
            get_calls: get_calls_for_provider,
            gate: gate_for_provider,
        }),
    );
    registry
        .list(SkillViewOptions {
            cwd: Some("/workspace/cache".to_string()),
            signal: None,
            scope: None,
        })
        .await
        .expect("warm");

    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_signal = flag.clone();
    let signal: SkillAbort = Arc::new(move || flag_for_signal.load(Ordering::SeqCst));
    let registry_for_task = registry.clone();
    let task = tokio::spawn(async move {
        registry_for_task
            .get(
                "cached-skill",
                SkillViewOptions {
                    cwd: Some("/workspace/cache".to_string()),
                    signal: Some(signal),
                    scope: None,
                },
            )
            .await
    });
    // Let the get reach the gated load, then arm the abort: the wait race
    // settles aborted before the held load resolves.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    flag.store(true, Ordering::SeqCst);
    let outcome = task.await.expect("join");
    assert_eq!(outcome.expect_err("aborted"), SKILL_ABORTED_MESSAGE);
    // Deviation: the TS recheck lands before the provider call (getCalls 0);
    // the Rust wait boundary polls the already-called load future, so the
    // counter reads 1 — the load never COMPLETES either way.
    assert_eq!(get_calls.load(Ordering::SeqCst), 1);
    gate.notify_waiters();
}

#[tokio::test(flavor = "current_thread")]
async fn caches_provider_discovery_skips_failing_providers_and_invalidates_on_runtime_skills() {
    let (ctx, registry) = installed(Some(Config {
        collect_cache_max_entries: Some(1),
    }));
    let warns = capture_warns(&ctx);
    let provider = MemoryProvider::new(
        "memory",
        vec![memory_skill("first-skill", "First", 10, None)],
    );
    register_provider(&ctx, provider.clone());

    let names = |skills: Vec<dsh_skill::SkillSummary>| -> Vec<String> {
        skills.into_iter().map(|skill| skill.name).collect()
    };
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["first-skill"]
    );
    provider.replace(vec![memory_skill("second-skill", "Second", 10, None)]);
    // Cached catalog still serves the old entry.
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["first-skill"]
    );

    let dispose_runtime = registry.register(
        &ctx,
        SkillRegistration {
            resource_base: Some(SkillResourceBase::Opaque {
                description: "runtime memory".to_string(),
            }),
            path: Some("memory://runtime-skill".to_string()),
            metadata: Some(serde_json::json!({ "owner": "tests" })),
            content: "Runtime body.".to_string(),
            ..runtime_registration("runtime-skill", "Runtime")
        },
    );
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["runtime-skill", "second-skill"]
    );
    let loaded = registry
        .get("runtime-skill", SkillViewOptions::default())
        .await
        .expect("get");
    let loaded = loaded.expect("some");
    assert_eq!(loaded.content, "Runtime body.");
    assert_eq!(loaded.path.as_deref(), Some("memory://runtime-skill"));
    assert_eq!(
        loaded.metadata,
        Some(serde_json::json!({ "owner": "tests" }))
    );
    (dispose_runtime)().await;

    struct FlakyProvider {
        calls: AtomicU64,
        fail: AtomicBool,
    }
    #[async_trait::async_trait]
    impl SkillProvider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("transient discovery failure".to_string());
            }
            Ok(SkillProviderObservation {
                candidates: vec![{
                    let mut candidate = memory_skill("flaky-skill", "Flaky", 10, None);
                    candidate.provider = "flaky".to_string();
                    candidate
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    let flaky = Arc::new(FlakyProvider {
        calls: AtomicU64::new(0),
        fail: AtomicBool::new(true),
    });
    register_provider(&ctx, flaky.clone());
    let incomplete = registry
        .snapshot(SkillViewOptions::default())
        .await
        .expect("snapshot");
    assert_eq!(names(incomplete.skills), vec!["second-skill"]);
    assert!(!incomplete.complete);
    assert_eq!(flaky.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["second-skill"]
    );
    assert_eq!(flaky.calls.load(Ordering::SeqCst), 2);
    flaky.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["flaky-skill", "second-skill"]
    );
    assert_eq!(flaky.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        names(
            registry
                .list(SkillViewOptions::default())
                .await
                .expect("list")
        ),
        vec!["flaky-skill", "second-skill"]
    );
    assert_eq!(flaky.calls.load(Ordering::SeqCst), 3);
    assert!(
        warns
            .warns
            .lock()
            .iter()
            .any(|message| message.contains("transient discovery failure"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_candidates_from_incomplete_provider_observations_loadable_without_caching() {
    let (ctx, registry) = installed(None);
    let list_calls = Arc::new(AtomicU64::new(0));
    let list_calls_for_provider = list_calls.clone();
    struct IncompleteProvider {
        calls: Arc<AtomicU64>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for IncompleteProvider {
        fn name(&self) -> &str {
            "incomplete-candidates"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(SkillProviderObservation {
                candidates: vec![{
                    let mut candidate = memory_skill("available-skill", "Available", 10, None);
                    candidate.provider = "incomplete-candidates".to_string();
                    candidate
                }],
                complete: false,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    register_provider(
        &ctx,
        Arc::new(IncompleteProvider {
            calls: list_calls_for_provider,
        }),
    );

    let snapshot = registry
        .snapshot(SkillViewOptions::default())
        .await
        .expect("snapshot");
    assert_eq!(snapshot.skills[0].name, "available-skill");
    assert!(!snapshot.complete);
    let loaded = registry
        .get("available-skill", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(loaded.expect("some").content, "available-skill body.");
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed[0].name, "available-skill");
    assert_eq!(list_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn invalidates_only_the_exact_registered_provider_and_ignores_its_late_callbacks() {
    let (ctx, registry) = installed(None);
    let provider = MemoryProvider::new(
        "memory",
        vec![memory_skill("first-skill", "First", 10, None)],
    );
    let provider_for_replace = provider.clone();
    let invalidate: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let signal_seen: Arc<parking_lot::Mutex<Option<SkillAbort>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let invalidate_for_factory = invalidate.clone();
    let signal_for_factory = signal_seen.clone();
    let dispose = registry.register_provider(
        &ctx,
        Arc::new(move |control| {
            *invalidate_for_factory.lock() = Some(control.invalidate.clone());
            *signal_for_factory.lock() = Some(control.signal.clone());
            provider.clone()
        }),
    );

    assert!(
        registry
            .snapshot(SkillViewOptions::default())
            .await
            .expect("snapshot")
            .complete
    );
    provider_for_replace.replace(vec![memory_skill("second-skill", "Second", 10, None)]);
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed[0].name, "first-skill");

    (invalidate.lock().as_ref().expect("control").clone())();
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed[0].name, "second-skill");
    (dispose)().await;
    assert!(signal_seen.lock().as_ref().expect("signal")());

    let replacement = MemoryProvider::new(
        "memory",
        vec![memory_skill("replacement-skill", "Replacement", 10, None)],
    );
    register_provider(&ctx, replacement.clone());
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed[0].name, "replacement-skill");
    // The stale control no longer invalidates the new registration.
    (invalidate.lock().as_ref().expect("control").clone())();
    let listed = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed[0].name, "replacement-skill");
    assert_eq!(replacement.list_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn emits_catalog_invalidations_for_live_provider_and_runtime_mutations() {
    let (ctx, registry) = installed(None);
    let changes = Arc::new(AtomicU64::new(0));
    let changes_for_listener = changes.clone();
    let _ = futures::executor::block_on(ctx.on(
        "skills/change",
        Arc::new(move |_ctx, _args| {
            let changes = changes_for_listener.clone();
            Box::pin(async move {
                changes.fetch_add(1, Ordering::SeqCst);
                None
            })
        }),
        cordis::EventOptions::default(),
    ));
    let provider = MemoryProvider::new(
        "memory",
        vec![memory_skill("provider-skill", "Provider", 10, None)],
    );
    let invalidate: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let invalidate_for_factory = invalidate.clone();
    let dispose_provider = registry.register_provider(
        &ctx,
        Arc::new(move |control| {
            *invalidate_for_factory.lock() = Some(control.invalidate.clone());
            provider.clone()
        }),
    );
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    (invalidate.lock().as_ref().expect("control").clone())();
    assert_eq!(changes.load(Ordering::SeqCst), 2);

    let dispose_runtime = registry.register(&ctx, runtime_registration("runtime-skill", "Runtime"));
    assert_eq!(changes.load(Ordering::SeqCst), 3);
    (dispose_runtime)().await;
    assert_eq!(changes.load(Ordering::SeqCst), 4);
    (dispose_provider)().await;
    assert_eq!(changes.load(Ordering::SeqCst), 5);
    (invalidate.lock().as_ref().expect("control").clone())();
    assert_eq!(changes.load(Ordering::SeqCst), 5);
}

#[tokio::test(flavor = "current_thread")]
async fn contains_synchronous_catalog_observer_failures() {
    let (ctx, _registry) = installed(None);
    let warns = capture_warns(&ctx);
    let observed = Arc::new(AtomicU64::new(0));
    let observed_for_listener = observed.clone();
    let _ = futures::executor::block_on(ctx.on(
        "skills/change",
        Arc::new(|_ctx, _args| panic!("observer threw")),
        cordis::EventOptions::default(),
    ));
    let _ = futures::executor::block_on(ctx.on(
        "skills/change",
        Arc::new(move |_ctx, _args| {
            let observed = observed_for_listener.clone();
            Box::pin(async move {
                observed.fetch_add(1, Ordering::SeqCst);
                None
            })
        }),
        cordis::EventOptions::default(),
    ));

    let provider = MemoryProvider::new("memory", Vec::new());
    register_provider(&ctx, provider);
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert!(
        warns
            .warns
            .lock()
            .iter()
            .any(|message| message.contains("skills/change listener threw"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retries_an_in_flight_catalog_invalidated_by_its_provider() {
    let (ctx, registry) = installed(None);
    let gate = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    struct GatedProvider {
        candidates: parking_lot::Mutex<Vec<SkillCandidate>>,
        calls: AtomicU64,
        gate: Arc<tokio::sync::Notify>,
        started: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for GatedProvider {
        fn name(&self) -> &str {
            "memory"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                self.started.notify_waiters();
                self.gate.notified().await;
            }
            Ok(SkillProviderObservation {
                candidates: self.candidates.lock().clone(),
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    let provider = Arc::new(GatedProvider {
        candidates: parking_lot::Mutex::new(vec![memory_skill("stale-skill", "Stale", 10, None)]),
        calls: AtomicU64::new(0),
        gate: gate.clone(),
        started: started.clone(),
    });
    let provider_for_replace = provider.clone();
    let invalidate: Arc<parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let invalidate_for_factory = invalidate.clone();
    registry.register_provider(
        &ctx,
        Arc::new(move |control| {
            *invalidate_for_factory.lock() = Some(control.invalidate.clone());
            provider.clone()
        }),
    );

    let registry_for_task = registry.clone();
    let task =
        tokio::spawn(async move { registry_for_task.list(SkillViewOptions::default()).await });
    started.notified().await;
    *provider_for_replace.candidates.lock() = vec![memory_skill("fresh-skill", "Fresh", 10, None)];
    (invalidate.lock().as_ref().expect("control").clone())();
    gate.notify_waiters();

    let listed = task.await.expect("join").expect("list");
    assert_eq!(listed[0].name, "fresh-skill");
    assert_eq!(provider_for_replace.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn bounds_repeated_in_flight_invalidation_and_leaves_the_result_uncached() {
    let (ctx, registry) = installed(None);
    let calls = Arc::new(AtomicU64::new(0));
    let calls_for_provider = calls.clone();
    struct SelfInvalidating {
        calls: Arc<AtomicU64>,
        invalidate: parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for SelfInvalidating {
        fn name(&self) -> &str {
            "self-invalidating"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(invalidate) = self.invalidate.lock().as_ref().cloned() {
                invalidate();
            }
            Ok(SkillProviderObservation {
                candidates: vec![{
                    let mut candidate =
                        memory_skill("bounded-skill", &format!("Attempt {call}"), 10, None);
                    candidate.provider = "self-invalidating".to_string();
                    candidate
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    let provider = Arc::new(SelfInvalidating {
        calls: calls_for_provider,
        invalidate: parking_lot::Mutex::new(None),
    });
    let provider_for_factory = provider.clone();
    registry.register_provider(
        &ctx,
        Arc::new(move |control| {
            *provider_for_factory.invalidate.lock() = Some(control.invalidate.clone());
            provider_for_factory.clone()
        }),
    );

    let snapshot = registry
        .snapshot(SkillViewOptions::default())
        .await
        .expect("snapshot");
    assert_eq!(snapshot.skills[0].description, "Attempt 2");
    assert!(!snapshot.complete);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let snapshot = registry
        .snapshot(SkillViewOptions::default())
        .await
        .expect("snapshot");
    assert_eq!(snapshot.skills[0].description, "Attempt 4");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn invalidates_a_provider_whose_loaded_definition_changed_identity() {
    let (ctx, registry) = installed(None);
    let calls = Arc::new(AtomicU64::new(0));
    let calls_for_provider = calls.clone();
    struct RenamedProvider {
        calls: Arc<AtomicU64>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for RenamedProvider {
        fn name(&self) -> &str {
            "renamed"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "old-name".to_string(),
                    description: "Old name".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "renamed".to_string(),
                    source: "test".to_string(),
                    resource_base: None,
                    rank: 1,
                    locator: arc(Locator {
                        content: "body".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(Some(SkillDefinition {
                name: "new-name".to_string(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: "Fresh body.".to_string(),
                path: None,
                metadata: None,
            }))
        }
    }
    register_provider(
        &ctx,
        Arc::new(RenamedProvider {
            calls: calls_for_provider,
        }),
    );

    assert!(
        registry
            .get("old-name", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );
    let _ = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn returns_none_when_a_discovered_candidate_disappears_before_loading() {
    let (ctx, registry) = installed(None);
    struct VanishedProvider;
    #[async_trait::async_trait]
    impl SkillProvider for VanishedProvider {
        fn name(&self) -> &str {
            "vanished-body"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![{
                    let mut candidate = memory_skill("vanished-skill", "Vanished", 10, None);
                    candidate.provider = "vanished-body".to_string();
                    candidate
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    register_provider(&ctx, Arc::new(VanishedProvider));

    assert!(
        registry
            .get("vanished-skill", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_a_load_failure_raced_against_an_armed_abort_signal() {
    let (ctx, registry) = installed(None);
    struct FailingLoader;
    #[async_trait::async_trait]
    impl SkillProvider for FailingLoader {
        fn name(&self) -> &str {
            "failing-loader"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "failing-skill".to_string(),
                    description: "Failing".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "failing-loader".to_string(),
                    source: "test".to_string(),
                    resource_base: None,
                    rank: 10,
                    locator: arc(Locator {
                        content: "x".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Err("load failed".to_string())
        }
    }
    register_provider(&ctx, Arc::new(FailingLoader));

    let error = registry
        .get(
            "failing-skill",
            SkillViewOptions {
                cwd: None,
                signal: Some(never_abort()),
                scope: None,
            },
        )
        .await
        .expect_err("load failure");
    assert_eq!(error, "load failed");
}

#[tokio::test(flavor = "current_thread")]
async fn abandons_an_in_flight_catalog_when_provider_registrations_change() {
    let (ctx, registry) = installed(None);
    let gate = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    struct DelayedProvider {
        gate: Arc<tokio::sync::Notify>,
        started: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for DelayedProvider {
        fn name(&self) -> &str {
            "delayed"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            self.started.notify_waiters();
            self.gate.notified().await;
            Ok(SkillProviderObservation {
                candidates: vec![{
                    let mut candidate = memory_skill("stale-skill", "Stale", 10, None);
                    candidate.provider = "delayed".to_string();
                    candidate
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    let dispose = register_provider(
        &ctx,
        Arc::new(DelayedProvider {
            gate: gate.clone(),
            started: started.clone(),
        }),
    );

    let registry_for_task = registry.clone();
    let task =
        tokio::spawn(async move { registry_for_task.list(SkillViewOptions::default()).await });
    started.notified().await;
    (dispose)().await;
    gate.notify_waiters();

    let listed = task.await.expect("join").expect("list");
    assert!(listed.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stops_waiting_for_discovery_when_its_lookup_signal_aborts() {
    let (ctx, registry) = installed(None);
    let gate = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let seen_signal: Arc<parking_lot::Mutex<Option<SkillAbort>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let seen_for_provider = seen_signal.clone();
    struct UncooperativeProvider {
        gate: Arc<tokio::sync::Notify>,
        started: Arc<tokio::sync::Notify>,
        seen: Arc<parking_lot::Mutex<Option<SkillAbort>>>,
    }
    #[async_trait::async_trait]
    impl SkillProvider for UncooperativeProvider {
        fn name(&self) -> &str {
            "uncooperative"
        }
        async fn list(
            &self,
            options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            *self.seen.lock() = options.signal.clone();
            self.started.notify_waiters();
            self.gate.notified().await;
            Ok(SkillProviderObservation::default())
        }
        async fn get(
            &self,
            _candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            Ok(None)
        }
    }
    register_provider(
        &ctx,
        Arc::new(UncooperativeProvider {
            gate: gate.clone(),
            started: started.clone(),
            seen: seen_for_provider,
        }),
    );

    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_signal = flag.clone();
    let signal: SkillAbort = Arc::new(move || flag_for_signal.load(Ordering::SeqCst));
    let registry_for_task = registry.clone();
    let signal_for_task = signal.clone();
    let task = tokio::spawn(async move {
        registry_for_task
            .list(SkillViewOptions {
                cwd: None,
                signal: Some(signal_for_task),
                scope: None,
            })
            .await
    });
    started.notified().await;
    flag.store(true, Ordering::SeqCst);
    let outcome = task.await.expect("join");
    assert_eq!(outcome.expect_err("aborted"), SKILL_ABORTED_MESSAGE);
    gate.notify_waiters();
    assert!(Arc::ptr_eq(
        seen_signal.lock().as_ref().expect("seen"),
        &signal
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_runtime_skill_registrations_and_ignores_duplicates() {
    let (ctx, registry) = installed(None);
    let warns = capture_warns(&ctx);
    let bad_name = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register(&ctx, runtime_registration("Bad_Name", "Bad"));
    }));
    assert!(bad_name.is_err(), "invalid name fails loud");
    let no_description = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register(&ctx, runtime_registration("no-description", ""));
    }));
    assert!(no_description.is_err(), "empty description fails loud");
    assert!(
        registry
            .get("missing-skill", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        registry
            .get("Bad_Name", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );

    let dispose_first = registry.register(&ctx, runtime_registration("same-skill", "First"));
    let dispose_second = registry.register(&ctx, runtime_registration("same-skill", "Second"));
    (dispose_second)().await;
    let loaded = registry
        .get("same-skill", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(loaded.expect("some").description, "First");
    (dispose_first)().await;
    assert!(
        registry
            .get("same-skill", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        warns
            .warns
            .lock()
            .iter()
            .any(|message| message.contains("ignored because it is already registered"))
    );
}

// ---- renderSkillContent ----

#[test]
fn renders_a_directory_based_skill_with_the_shared_wrapper() {
    let text = render_skill_content(
        "demo-skill",
        "memory",
        Some(&SkillResourceBase::Directory {
            path: "/tmp/demo".to_string(),
        }),
        "Do the thing.",
    );
    assert_eq!(
        text,
        [
            "<skill_content name=\"demo-skill\">",
            "<skill_resources>",
            "Base directory for this skill: /tmp/demo",
            "Resolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.",
            "</skill_resources>",
            "",
            "<skill_instructions>",
            "Do the thing.",
            "</skill_instructions>",
            "</skill_content>",
        ]
        .join("\n")
    );
}

#[test]
fn renders_url_and_opaque_resource_hints() {
    let url = render_skill_content(
        "url-skill",
        "memory",
        Some(&SkillResourceBase::Url {
            url: "https://example.test/base/".to_string(),
        }),
        "Body.",
    );
    assert!(url.contains("Base URL for this skill: https://example.test/base/"));
    assert!(url.contains(
        "Resolve relative URLs mentioned by this skill against the base URL before using them."
    ));

    let opaque = render_skill_content(
        "opaque-skill",
        "memory",
        Some(&SkillResourceBase::Opaque {
            description: "archive <bundle>".to_string(),
        }),
        "Body.",
    );
    assert!(opaque.contains("Resources for this skill: archive &lt;bundle&gt;"));
}

#[test]
fn falls_back_to_the_provider_hint_without_a_resource_base() {
    let text = render_skill_content("provider-skill", "remote <hub>", None, "Body.");
    assert!(
        text.contains("Resources for this skill are managed by provider \"remote &lt;hub&gt;\".")
    );
}

#[test]
fn escapes_hostile_attribute_names_and_keeps_the_body_verbatim() {
    let text = render_skill_content(
        "x\"&<y",
        "memory",
        Some(&SkillResourceBase::Directory {
            path: "/tmp".to_string(),
        }),
        "Keep </skill_content> and <tags> as-is.",
    );
    assert!(text.contains("<skill_content name=\"x&quot;&amp;&lt;y\">"));
    assert!(text.contains("Keep </skill_content> and <tags> as-is."));
    assert_eq!(escape_text("a&<b>"), "a&amp;&lt;b&gt;");
    assert!(is_skill_name("demo-skill"));
    assert!(!is_skill_name("Bad_Name"));
    assert_eq!(BUNDLED_SKILL_RANK, 600);
}

// ---- scoped layers ----

#[tokio::test(flavor = "current_thread")]
async fn files_a_scoped_provider_into_its_layer_and_merges_it_into_that_scope_view_only() {
    let (ctx, registry) = installed(None);
    register_provider(
        &ctx,
        MemoryProvider::new(
            "memory",
            vec![memory_skill("global-skill", "Global", 100, None)],
        ),
    );
    let preset = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    struct PresetProvider;
    #[async_trait::async_trait]
    impl SkillProvider for PresetProvider {
        fn name(&self) -> &str {
            "preset-local"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "preset-skill".to_string(),
                    description: "Preset".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "preset-local".to_string(),
                    source: "preset".to_string(),
                    resource_base: None,
                    rank: 300,
                    locator: arc(Locator {
                        content: "Preset body.".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    let _ = registry.register_provider(
        &preset.ctx,
        Arc::new(move |_control| Arc::new(PresetProvider)),
    );

    let global = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let global_names: Vec<&str> = global.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(global_names, vec!["global-skill"]);
    let scope = dsh_scope::scope_of(&preset.ctx);
    let scoped = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope,
        })
        .await
        .expect("list");
    let scoped_names: Vec<&str> = scoped.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(scoped_names, vec!["global-skill", "preset-skill"]);
    let scope = dsh_scope::scope_of(&preset.ctx);
    let loaded = registry
        .get(
            "preset-skill",
            SkillViewOptions {
                cwd: None,
                signal: None,
                scope,
            },
        )
        .await
        .expect("get");
    assert_eq!(loaded.expect("some").content, "Preset body.");
    assert!(
        registry
            .get("preset-skill", SkillViewOptions::default())
            .await
            .expect("get")
            .is_none()
    );
    (preset.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn lets_the_nearest_layer_win_a_duplicate_name_regardless_of_rank() {
    let (ctx, registry) = installed(None);
    register_provider(
        &ctx,
        MemoryProvider::new(
            "memory",
            vec![memory_skill("shared-name", "Global wins ranks", 10, None)],
        ),
    );
    let preset = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    struct ShadowProvider;
    #[async_trait::async_trait]
    impl SkillProvider for ShadowProvider {
        fn name(&self) -> &str {
            "preset-local"
        }
        async fn list(
            &self,
            _options: &SkillLookupOptions,
        ) -> Result<SkillProviderObservation, String> {
            Ok(SkillProviderObservation {
                candidates: vec![SkillCandidate {
                    name: "shared-name".to_string(),
                    description: "Preset shadow".to_string(),
                    when_to_use: None,
                    invocation: SkillInvocationPolicy::BOTH,
                    provider: "preset-local".to_string(),
                    source: "preset".to_string(),
                    resource_base: None,
                    rank: 900,
                    locator: arc(Locator {
                        content: "Preset shadow body.".to_string(),
                    }),
                    path: None,
                    metadata: None,
                }],
                complete: true,
            })
        }
        async fn get(
            &self,
            candidate: &SkillCandidate,
            _options: &SkillLookupOptions,
        ) -> Result<Option<SkillDefinition>, String> {
            let locator = downcast::<Locator>(&candidate.locator).expect("locator");
            Ok(Some(SkillDefinition {
                name: candidate.name.clone(),
                description: candidate.description.clone(),
                when_to_use: None,
                invocation: candidate.invocation,
                source: candidate.source.clone(),
                provider: candidate.provider.clone(),
                resource_base: None,
                content: locator.content.clone(),
                path: None,
                metadata: None,
            }))
        }
    }
    let _ = registry.register_provider(
        &preset.ctx,
        Arc::new(move |_control| Arc::new(ShadowProvider)),
    );

    let scope = dsh_scope::scope_of(&preset.ctx);
    let scoped = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: scope.clone(),
        })
        .await
        .expect("list");
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].description, "Preset shadow");
    let loaded = registry
        .get(
            "shared-name",
            SkillViewOptions {
                cwd: None,
                signal: None,
                scope,
            },
        )
        .await
        .expect("get");
    assert_eq!(loaded.expect("some").content, "Preset shadow body.");
    let global = registry
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(global[0].description, "Global wins ranks");
    (preset.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn resolves_the_scope_chain_and_recompose_follows_the_new_parent() {
    let (ctx, registry) = installed(None);
    let preset_a = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let preset_b = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let _ = registry.register(&preset_a.ctx, runtime_registration("skill-a", "Skill a"));
    let _ = registry.register(&preset_b.ctx, runtime_registration("skill-b", "Skill b"));
    let agent_key = ScopeKey::new();
    let binding = bind_scope_parent(
        &agent_key,
        &dsh_scope::scope_of(&preset_a.ctx).expect("key a"),
    );
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: Some(agent_key.clone()),
        })
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["skill-a"]);
    binding.rebind(&dsh_scope::scope_of(&preset_b.ctx).expect("key b"));
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: Some(agent_key),
        })
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["skill-b"]);
    (preset_a.dispose)().await;
    (preset_b.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn scopes_provider_name_uniqueness_per_layer_and_reports_scoped_duplicates_distinctly() {
    let (ctx, registry) = installed(None);
    register_provider(&ctx, MemoryProvider::new("memory", Vec::new()));
    let preset_a = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let preset_b = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let _ = registry.register_provider(
        &preset_a.ctx,
        Arc::new(|_control| {
            MemoryProvider::new("memory", vec![memory_skill("a-only", "A", 100, None)])
        }),
    );
    let _ = registry.register_provider(
        &preset_b.ctx,
        Arc::new(|_control| {
            MemoryProvider::new("memory", vec![memory_skill("b-only", "B", 100, None)])
        }),
    );
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = registry.register_provider(
            &preset_a.ctx,
            Arc::new(|_control| MemoryProvider::new("memory", Vec::new())),
        );
    }));
    assert!(duplicate.is_err(), "scoped duplicate fails loud");
    let scope = dsh_scope::scope_of(&preset_a.ctx);
    let listed_a = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope,
        })
        .await
        .expect("list");
    assert_eq!(listed_a[0].name, "a-only");
    let scope = dsh_scope::scope_of(&preset_b.ctx);
    let listed_b = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope,
        })
        .await
        .expect("list");
    assert_eq!(listed_b[0].name, "b-only");
    (preset_a.dispose)().await;
    (preset_b.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_runtime_duplicate_handling_per_layer_and_shadows_a_global_runtime_name() {
    let (ctx, registry) = installed(None);
    let warns = capture_warns(&ctx);
    let _ = registry.register(
        &ctx,
        SkillRegistration {
            content: "Global body.".to_string(),
            description: "Global runtime".to_string(),
            ..runtime_registration("told-twice", "Global runtime")
        },
    );
    let preset = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let dispose_shadow = registry.register(
        &preset.ctx,
        SkillRegistration {
            source: "preset".to_string(),
            content: "Preset body.".to_string(),
            description: "Preset runtime".to_string(),
            ..runtime_registration("told-twice", "Preset runtime")
        },
    );
    assert!(
        !warns
            .warns
            .lock()
            .iter()
            .any(|message| message.contains("told-twice"))
    );
    let _ = registry.register(
        &preset.ctx,
        SkillRegistration {
            source: "preset".to_string(),
            content: "Ignored.".to_string(),
            description: "Ignored".to_string(),
            ..runtime_registration("told-twice", "Ignored")
        },
    );
    assert!(warns.warns.lock().iter().any(|message| message
        == "runtime skill \"told-twice\" ignored because it is already registered"));
    let scope = dsh_scope::scope_of(&preset.ctx);
    let scoped = registry
        .get(
            "told-twice",
            SkillViewOptions {
                cwd: None,
                signal: None,
                scope,
            },
        )
        .await
        .expect("get");
    assert_eq!(scoped.expect("some").content, "Preset body.");
    let global = registry
        .get("told-twice", SkillViewOptions::default())
        .await
        .expect("get");
    assert_eq!(global.expect("some").content, "Global body.");
    (dispose_shadow)().await;
    let scope = dsh_scope::scope_of(&preset.ctx);
    let scoped = registry
        .get(
            "told-twice",
            SkillViewOptions {
                cwd: None,
                signal: None,
                scope,
            },
        )
        .await
        .expect("get");
    assert_eq!(scoped.expect("some").content, "Global body.");
    (preset.dispose)().await;
}

#[tokio::test(flavor = "current_thread")]
async fn drops_a_disposed_scoped_registration_from_its_scope_view_and_notifies_change() {
    let (ctx, registry) = installed(None);
    let changes = Arc::new(AtomicU64::new(0));
    let changes_for_listener = changes.clone();
    let _ = futures::executor::block_on(ctx.on(
        "skills/change",
        Arc::new(move |_ctx, _args| {
            let changes = changes_for_listener.clone();
            Box::pin(async move {
                changes.fetch_add(1, Ordering::SeqCst);
                None
            })
        }),
        cordis::EventOptions::default(),
    ));
    let preset = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let _ = registry.register_provider(
        &preset.ctx,
        Arc::new(|_control| {
            MemoryProvider::new(
                "memory",
                vec![memory_skill("scoped-skill", "Scoped", 100, None)],
            )
        }),
    );
    let scope = dsh_scope::scope_of(&preset.ctx);
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: scope.clone(),
        })
        .await
        .expect("list");
    assert_eq!(listed[0].name, "scoped-skill");
    let notified = changes.load(Ordering::SeqCst);
    // The scope fiber's apply chain runs as spawned lifecycle work; settle
    // the runtime first so disposal unloads (and runs the registration
    // undo) instead of joining a still-queued apply.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (preset.dispose)().await;
    assert!(changes.load(Ordering::SeqCst) > notified);
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope,
        })
        .await
        .expect("list");
    assert!(listed.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn invalidates_through_a_scoped_provider_control_only_while_its_exact_registration_is_live() {
    let (ctx, registry) = installed(None);
    let preset = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let provider = MemoryProvider::new(
        "memory",
        vec![memory_skill("watched", "Watched", 100, None)],
    );
    let control: Arc<parking_lot::Mutex<Option<SkillProviderControl>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let control_for_factory = control.clone();
    let provider_for_factory = provider.clone();
    let dispose = registry.register_provider(
        &preset.ctx,
        Arc::new(move |given| {
            *control_for_factory.lock() = Some(given);
            provider_for_factory.clone()
        }),
    );
    let scope = dsh_scope::scope_of(&preset.ctx);
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: scope.clone(),
        })
        .await
        .expect("list");
    assert_eq!(listed[0].name, "watched");
    provider.replace(vec![memory_skill("replaced", "Replaced", 100, None)]);
    (control.lock().as_ref().expect("control").invalidate)();
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope: scope.clone(),
        })
        .await
        .expect("list");
    assert_eq!(listed[0].name, "replaced");
    (dispose)().await;
    provider.replace(vec![memory_skill("ignored", "Ignored", 100, None)]);
    (control.lock().as_ref().expect("control").invalidate)();
    let listed = registry
        .list(SkillViewOptions {
            cwd: None,
            signal: None,
            scope,
        })
        .await
        .expect("list");
    assert!(listed.is_empty());
    (preset.dispose)().await;
}

#[test]
fn rejects_invalid_registry_caps() {
    let ctx = Context::root();
    let error = SkillRegistry::install(
        &ctx,
        Config {
            collect_cache_max_entries: Some(0),
        },
    )
    .err()
    .expect("invalid cap");
    assert!(error.contains("collectCacheMaxEntries"), "{error}");
}
