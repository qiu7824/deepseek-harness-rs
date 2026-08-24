#![allow(clippy::type_complexity, clippy::explicit_counter_loop)]
// Provider invalidation callbacks intentionally retain their public shape.

//! Agent skill provider registry.
//!
//! This package owns the Service Definition role of the skill capability
//! seam. Concrete providers such as `@deepseek-ai/dsh-skill-filesystem`
//! decide where skills come from; this service only merges provider
//! catalogs, resolves the winning skill for a name, and exposes the winning
//! summaries and definitions to consumers.
//! Rust port of `packages/skill/skill/src/index.ts`.
//!
//! # Deviations
//!
//! - The abort seam is a predicate without a reason payload; an aborted
//!   lookup or load resolves `Err(SKILL_ABORTED_MESSAGE)`.
//! - `SkillProvider::list` returns a closed
//!   [`SkillProviderObservation`] (an array-shorthand is the same struct
//!   with `complete: true`), so the TS malformed-observation and
//!   malformed-scalar runtime validations are compile-time facts here; the
//!   name-grammar, non-empty-description, and provider-ownership checks
//!   remain runtime.
//! - `skills/change` listeners run inline with per-listener panic
//!   containment and a `skills/change listener threw` warning; the Rust
//!   listener signature has no rejection channel, so the TS
//!   listener-rejected warning has no counterpart.
//! - The plugin config schema is an in-code check (the loader integration
//!   for this seam is not wired yet).

pub mod invariant;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cordis::{ArcValue, Context, DispatchMode, Disposer, Plugin, PluginError, arc, downcast};
use dsh_scope::{NamedEntries, ScopeKey, ScopeLayer, ScopedLayers, scope_chain_of, scope_of};
use indexmap::IndexMap;

const SKILL_NAME_PATTERN: &str = r"^[a-z0-9]+(-[a-z0-9]+)*$";
const DEFAULT_COLLECT_CACHE_ENTRIES: usize = 128;
const MAX_COLLECT_ATTEMPTS: usize = 2;
const RUNTIME_PROVIDER: &str = "runtime";
const RUNTIME_RANK: i64 = 250;

/// Standard precedence rank for packaged skill providers and local bundled
/// roots.
pub const BUNDLED_SKILL_RANK: i64 = 600;

/// The uniform abort message (TS carries the caller's abort reason; the
/// Rust predicate has no payload).
pub const SKILL_ABORTED_MESSAGE: &str = "skill lookup aborted";

/// Return whether a string is a valid kebab-case skill name.
pub fn is_skill_name(name: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(SKILL_NAME_PATTERN).expect("static pattern"));
    PATTERN.is_match(name)
}

/// The cancellation seam (TS `AbortSignal`).
pub type SkillAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Invocation controls shared by skill discovery consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillInvocationPolicy {
    /// Whether model-facing catalogs and loaders include this skill.
    pub model_invocable: bool,
    /// Whether human-facing command catalogs and loaders include this
    /// skill.
    pub user_invocable: bool,
}

impl SkillInvocationPolicy {
    pub const BOTH: SkillInvocationPolicy = SkillInvocationPolicy {
        model_invocable: true,
        user_invocable: true,
    };
}

/// Optional provider-specific base used by loaded skill bodies to resolve
/// relative resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillResourceBase {
    Directory { path: String },
    Url { url: String },
    Opaque { description: String },
}

/// Invocation-neutral skill metadata returned by `ctx.skills.list()`.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillSummary {
    /// Kebab-case identifier used to address the skill.
    pub name: String,
    /// Short routing description shown by discovery consumers.
    pub description: String,
    /// Optional extra routing guidance.
    pub when_to_use: Option<String>,
    /// Resolved model and user invocation controls.
    pub invocation: SkillInvocationPolicy,
    /// Discovery source that produced this winning skill.
    pub source: String,
    /// Provider that owns this skill body.
    pub provider: String,
    /// Provider-specific base for relative resources.
    pub resource_base: Option<SkillResourceBase>,
}

/// Provider catalog entry used by the registry to merge and later load
/// skills.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub invocation: SkillInvocationPolicy,
    pub source: String,
    pub provider: String,
    pub resource_base: Option<SkillResourceBase>,
    /// Lower ranks win duplicate skill names before provider registration
    /// order is considered.
    pub rank: i64,
    /// Opaque provider-owned handle passed back to `provider.get()`.
    pub locator: ArcValue,
    /// Absolute file path when the provider has one.
    pub path: Option<String>,
    /// Parsed optional metadata object from provider-specific skill
    /// frontmatter.
    pub metadata: Option<serde_json::Value>,
}

/// Complete parsed skill definition, including the body loaded by
/// `ctx.skills.get()`.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub invocation: SkillInvocationPolicy,
    pub source: String,
    pub provider: String,
    pub resource_base: Option<SkillResourceBase>,
    /// Markdown instruction body after any provider-specific metadata
    /// removal.
    pub content: String,
    /// Absolute file path when the skill came from disk.
    pub path: Option<String>,
    /// Parsed optional metadata object from frontmatter.
    pub metadata: Option<serde_json::Value>,
}

/// Runtime skill contribution accepted by `ctx.skills.register()`.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRegistration {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub source: String,
    pub resource_base: Option<SkillResourceBase>,
    pub content: String,
    pub path: Option<String>,
    pub metadata: Option<serde_json::Value>,
    /// Invocation controls; omission permits both model and user surfaces.
    pub invocation: Option<SkillInvocationPolicy>,
    /// Provider label; omission uses the registry-owned runtime provider.
    pub provider: Option<String>,
}

/// Caller context used for cwd-sensitive and abortable provider work.
#[derive(Clone, Default)]
pub struct SkillLookupOptions {
    /// Workspace selector for the current lookup.
    pub cwd: Option<String>,
    /// Abort discovery or loading work for the current caller.
    pub signal: Option<SkillAbort>,
}

/// Registry read options: provider lookup context plus the viewing scope.
#[derive(Clone, Default)]
pub struct SkillViewOptions {
    /// Workspace selector for the current lookup.
    pub cwd: Option<String>,
    /// Abort discovery or loading work for the current caller.
    pub signal: Option<SkillAbort>,
    /// Viewing scope (the calling agent); omitted reads the global layer
    /// alone.
    pub scope: Option<ScopeKey>,
}

impl SkillViewOptions {
    fn lookup(&self) -> SkillLookupOptions {
        SkillLookupOptions {
            cwd: self.cwd.clone(),
            signal: self.signal.clone(),
        }
    }
}

/// Return whether a skill may be advertised to and loaded by a model.
pub fn is_model_invocable(skill: &SkillSummary) -> bool {
    skill.invocation.model_invocable
}

/// Return whether a skill may be advertised to and loaded by a
/// human-facing command.
pub fn is_user_invocable(skill: &SkillSummary) -> bool {
    skill.invocation.user_invocable
}

/// Render one loaded skill for the model (TS `renderSkillContent`). The
/// name rides an escaped attribute; the body is embedded verbatim.
pub fn render_skill_content(
    name: &str,
    provider: &str,
    resource_base: Option<&SkillResourceBase>,
    content: &str,
) -> String {
    let resource_hint = render_resource_hint(provider, resource_base);
    [
        format!("<skill_content name=\"{}\">", escape_attr(name)),
        "<skill_resources>".to_string(),
        resource_hint,
        "</skill_resources>".to_string(),
        String::new(),
        "<skill_instructions>".to_string(),
        content.to_string(),
        "</skill_instructions>".to_string(),
        "</skill_content>".to_string(),
    ]
    .join("\n")
}

fn render_resource_hint(provider: &str, base: Option<&SkillResourceBase>) -> String {
    match base {
        None => {
            format!(
                "Resources for this skill are managed by provider \"{}\".\nLoad referenced resources only as needed.",
                escape_text(provider)
            )
        }
        Some(SkillResourceBase::Directory { path }) => {
            format!(
                "Base directory for this skill: {}\nResolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.",
                escape_text(path)
            )
        }
        Some(SkillResourceBase::Url { url }) => {
            format!(
                "Base URL for this skill: {}\nResolve relative URLs mentioned by this skill against the base URL before using them. Load referenced resources only as needed.",
                escape_text(url)
            )
        }
        Some(SkillResourceBase::Opaque { description }) => {
            format!(
                "Resources for this skill: {}\nLoad referenced resources only as needed.",
                escape_text(description)
            )
        }
    }
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Escape model-facing prose embedded inside skill markup so
/// provider-supplied text cannot open or close framing tags.
pub fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One catalog observation plus whether discovery completed within a
/// stable catalog revision.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillCatalogSnapshot {
    /// Sorted invocation-neutral summaries collected in this observation.
    pub skills: Vec<SkillSummary>,
    /// Whether every registered provider completed without a concurrent
    /// catalog revision.
    pub complete: bool,
}

/// Provider candidates plus whether the current discovery is
/// authoritative (TS `SkillProviderObservation`; the array shorthand is
/// this struct with `complete: true`).
#[derive(Debug, Clone, Default)]
pub struct SkillProviderObservation {
    /// Candidates available from the current provider discovery.
    pub candidates: Vec<SkillCandidate>,
    /// Whether discovery completed and these candidates may be cached.
    pub complete: bool,
}

/// Provider interface for one source of skills, such as local directories
/// or a remote registry.
#[async_trait::async_trait]
pub trait SkillProvider: Send + Sync + 'static {
    /// Unique provider name in the `ctx.skills` registry.
    fn name(&self) -> &str;

    /// List available skill candidates for the current lookup context.
    /// Providers register synchronously during `apply()`; remote
    /// initialization, authentication, and discovery are awaited inside
    /// this method. Implementations should settle promptly when
    /// `options.signal` aborts.
    async fn list(&self, options: &SkillLookupOptions) -> Result<SkillProviderObservation, String>;

    /// Load a complete skill body for a previously listed candidate.
    async fn get(
        &self,
        candidate: &SkillCandidate,
        options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String>;
}

/// Registration-scoped lifecycle and invalidation capability borrowed by
/// one provider.
#[derive(Clone)]
pub struct SkillProviderControl {
    /// Aborts if registration fails or when the exact provider
    /// registration is disposed.
    pub signal: SkillAbort,
    /// Invalidate completed catalogs and notify consumers only while the
    /// exact registration remains active.
    pub invalidate: Arc<dyn Fn() + Send + Sync>,
}

/// Skill registry configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Maximum number of completed cwd/provider catalogs kept in memory.
    pub collect_cache_max_entries: Option<usize>,
}

/// One provider registration retained by its layer.
#[derive(Clone)]
pub struct RegisteredProvider {
    pub provider: Arc<dyn SkillProvider>,
    /// Service-wide monotonic registration order, the within-layer rank
    /// tiebreak.
    pub order: u64,
}

/// One scope's complete skill-registry contribution.
pub struct SkillLayer {
    /// Providers registered through contexts carrying this scope,
    /// insertion-ordered.
    pub providers: NamedEntries<RegisteredProvider>,
    /// Runtime skills registered through contexts carrying this scope. The
    /// map rides an `Arc` so layer-mutation undo closures stay `'static`.
    pub runtime: Arc<parking_lot::Mutex<IndexMap<String, SkillDefinition>>>,
}

impl SkillLayer {
    fn new(scope: Option<&ScopeKey>) -> Self {
        let scoped = scope.is_some();
        Self {
            providers: NamedEntries::new(
                move |name: &str| -> Box<dyn std::error::Error + Send + Sync> {
                    let message = if scoped {
                        format!(
                            "a skill provider named \"{name}\" is already registered in this scope"
                        )
                    } else {
                        format!("a skill provider named \"{name}\" is already registered")
                    };
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        message,
                    ))
                },
            ),
            runtime: Arc::new(parking_lot::Mutex::new(IndexMap::new())),
        }
    }
}

impl ScopeLayer for SkillLayer {
    fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.runtime.lock().is_empty()
    }
}

/// The registry-owned runtime provider: only owns `get()` (runtime skills
/// are injected directly).
struct RuntimeSkillProvider;

#[async_trait::async_trait]
impl SkillProvider for RuntimeSkillProvider {
    fn name(&self) -> &str {
        RUNTIME_PROVIDER
    }

    async fn list(
        &self,
        _options: &SkillLookupOptions,
    ) -> Result<SkillProviderObservation, String> {
        Ok(SkillProviderObservation::default())
    }

    async fn get(
        &self,
        candidate: &SkillCandidate,
        _options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        Ok(downcast::<SkillDefinition>(&candidate.locator).cloned())
    }
}

fn runtime_provider() -> Arc<dyn SkillProvider> {
    static PROVIDER: std::sync::LazyLock<Arc<dyn SkillProvider>> =
        std::sync::LazyLock::new(|| Arc::new(RuntimeSkillProvider));
    PROVIDER.clone()
}

struct IndexedCandidate {
    candidate: SkillCandidate,
    provider: Arc<dyn SkillProvider>,
    provider_order: i64,
    local_order: i64,
    /// Owning layer, so a stale-definition invalidation can verify the
    /// exact registration is still live.
    layer: Arc<SkillLayer>,
}

impl Clone for IndexedCandidate {
    fn clone(&self) -> Self {
        Self {
            candidate: self.candidate.clone(),
            provider: self.provider.clone(),
            provider_order: self.provider_order,
            local_order: self.local_order,
            layer: self.layer.clone(),
        }
    }
}

struct LayerCollectResult {
    entries: Vec<IndexedCandidate>,
    cacheable: bool,
}

struct CollectResult {
    entries: IndexMap<String, IndexedCandidate>,
    cacheable: bool,
}

/// Layered registry of skill providers, the host+per-scope shape the tools
/// registry established. A read merges the global layer with the viewing
/// scope's chain — the nearest layer's entry wins a duplicate name
/// outright, and the rank order decides duplicates only within one layer.
pub struct SkillRegistry {
    ctx: Context,
    collect_cache_max_entries: usize,
    layers: ScopedLayers<SkillLayer>,
    collect_cache: parking_lot::Mutex<IndexMap<String, IndexMap<String, IndexedCandidate>>>,
    revision: AtomicU64,
    next_provider_order: AtomicU64,
    /// Stable identities for cache keys (ScopeKey is identity-compared by
    /// its internal id, so the id map keys by value).
    scope_ids: parking_lot::Mutex<indexmap::IndexMap<ScopeKey, u64>>,
    next_scope_id: AtomicU64,
}

impl cordis::Service for SkillRegistry {
    fn service_name(&self) -> &'static str {
        "skills"
    }
}

impl SkillRegistry {
    /// Create the service and register it as `ctx.skills` (TS constructor).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let collect_cache_max_entries = config
            .collect_cache_max_entries
            .unwrap_or(DEFAULT_COLLECT_CACHE_ENTRIES);
        if collect_cache_max_entries < 1 {
            return Err(
                "skill: collectCacheMaxEntries must be an integer greater than or equal to 1"
                    .to_string(),
            );
        }
        // The layers' on-change callback invalidates the catalog cache; it
        // reaches the registry through a weak slot to break the
        // construction cycle (TS captures `this` before assignment).
        struct Slot(parking_lot::Mutex<Option<std::sync::Weak<SkillRegistry>>>);
        let slot = Arc::new(Slot(parking_lot::Mutex::new(None)));
        let layers = ScopedLayers::new(SkillLayer::new, {
            let slot = slot.clone();
            move || {
                if let Some(registry) = slot.0.lock().as_ref().and_then(std::sync::Weak::upgrade) {
                    registry.invalidate_cache();
                }
            }
        });
        let registry = Arc::new(Self {
            ctx: ctx.clone(),
            collect_cache_max_entries,
            layers,
            collect_cache: parking_lot::Mutex::new(IndexMap::new()),
            revision: AtomicU64::new(0),
            next_provider_order: AtomicU64::new(0),
            scope_ids: parking_lot::Mutex::new(IndexMap::new()),
            next_scope_id: AtomicU64::new(1),
        });
        *slot.0.lock() = Some(Arc::downgrade(&registry));
        ctx.register_service(registry.clone());
        Ok(registry)
    }

    /// Register a borrowed same-process provider synchronously during
    /// plugin apply, into the CALLING context's layer (the TS Proxy rebinds
    /// `this.ctx` to the caller). Duplicate names within one layer and
    /// reserved names panic; fiber disposal unregisters the provider and
    /// invalidates catalog caches.
    pub fn register_provider(
        self: &Arc<Self>,
        caller: &Context,
        create: Arc<dyn Fn(SkillProviderControl) -> Arc<dyn SkillProvider> + Send + Sync>,
    ) -> Disposer {
        let aborted = Arc::new(AtomicBool::new(false));
        // Liveness for the invalidate control: armed by the registration,
        // disarmed by its undo (a per-registration cell, so a stale control
        // from a disposed registration can never fire).
        let live = Arc::new(AtomicBool::new(false));
        let registry_weak = Arc::downgrade(self);
        let control = SkillProviderControl {
            signal: {
                let aborted = aborted.clone();
                Arc::new(move || aborted.load(Ordering::SeqCst))
            },
            invalidate: {
                let live = live.clone();
                let registry_weak = registry_weak.clone();
                Arc::new(move || {
                    if live.load(Ordering::SeqCst)
                        && let Some(registry) = registry_weak.upgrade()
                    {
                        registry.invalidate_cache();
                    }
                })
            },
        };
        let provider_handle = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (create)(control.clone())
        })) {
            Ok(handle) => handle,
            Err(payload) => {
                aborted.store(true, Ordering::SeqCst);
                std::panic::resume_unwind(payload);
            }
        };
        let name = provider_handle.name().to_string();
        if name == RUNTIME_PROVIDER {
            aborted.store(true, Ordering::SeqCst);
            panic!("\"{RUNTIME_PROVIDER}\" is reserved for runtime skill registrations");
        }
        let order = self.next_provider_order.fetch_add(1, Ordering::Relaxed);
        self.layers.effect(
            caller,
            move |layer| {
                let undo = layer.providers.insert(
                    &name,
                    RegisteredProvider {
                        provider: provider_handle.clone(),
                        order,
                    },
                );
                live.store(true, Ordering::SeqCst);
                let live_for_undo = live.clone();
                let aborted_for_dispose = aborted.clone();
                Box::new(move || {
                    live_for_undo.store(false, Ordering::SeqCst);
                    undo();
                    aborted_for_dispose.store(true, Ordering::SeqCst);
                })
            },
            "skills.registerProvider()",
            true,
        )
    }

    /// Register a borrowed readonly runtime skill into the CALLING
    /// context's layer (the TS Proxy rebinds `this.ctx` to the caller).
    /// Same-name runtime entries in one layer are first-wins; a duplicate
    /// logs a warning and receives a no-op disposer so it cannot remove the
    /// winner.
    pub fn register(&self, caller: &Context, skill: SkillRegistration) -> Disposer {
        validate_runtime_skill(&skill);
        let scope = scope_of(caller);
        let existing_layer = match &scope {
            None => Some(self.layers.global.clone()),
            Some(key) => self.layers.peek(Some(key)),
        };
        if existing_layer.is_some_and(|layer| layer.runtime.lock().contains_key(&skill.name)) {
            self.ctx.named_logger(None).warn(vec![arc(format!(
                "runtime skill \"{}\" ignored because it is already registered",
                skill.name
            ))]);
            return cordis::make_disposer(move || Box::pin(async {}));
        }
        let definition = SkillDefinition {
            name: skill.name.clone(),
            description: skill.description.clone(),
            when_to_use: skill.when_to_use.clone(),
            invocation: skill.invocation.unwrap_or(SkillInvocationPolicy::BOTH),
            source: skill.source.clone(),
            provider: skill
                .provider
                .clone()
                .unwrap_or_else(|| RUNTIME_PROVIDER.to_string()),
            resource_base: skill.resource_base.clone(),
            content: skill.content.clone(),
            path: skill.path.clone(),
            metadata: skill.metadata.clone(),
        };
        self.layers.effect(
            caller,
            move |layer| {
                let name = definition.name.clone();
                let runtime = layer.runtime.clone();
                layer
                    .runtime
                    .lock()
                    .insert(name.clone(), definition.clone());
                Box::new(move || {
                    runtime.lock().shift_remove(&name);
                })
            },
            "skills.register()",
            true,
        )
    }

    /// List invocation-neutral skill summaries for a workspace.
    pub async fn list(&self, options: SkillViewOptions) -> Result<Vec<SkillSummary>, String> {
        Ok(self.snapshot(options).await?.skills)
    }

    /// Observe the current invocation-neutral catalog and whether discovery
    /// completed within a stable revision.
    pub async fn snapshot(
        &self,
        options: SkillViewOptions,
    ) -> Result<SkillCatalogSnapshot, String> {
        let collected = self.collect(&options).await?;
        let mut skills: Vec<SkillSummary> = collected
            .entries
            .values()
            .map(|entry| to_summary(&entry.candidate))
            .collect();
        skills.sort_by(|left, right| compare_code_points(&left.name, &right.name));
        Ok(SkillCatalogSnapshot {
            skills,
            complete: collected.cacheable,
        })
    }

    /// Load and validate the winning candidate, passing its opaque
    /// discovery locator back to the provider.
    pub async fn get(
        &self,
        name: &str,
        options: SkillViewOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        if !is_skill_name(name) {
            return Ok(None);
        }
        let collected = self.collect(&options).await?;
        throw_if_aborted(options.signal.as_ref())?;
        let Some(entry) = collected.entries.get(name) else {
            return Ok(None);
        };
        let definition = wait_with_abort(
            entry.provider.get(&entry.candidate, &options.lookup()),
            options.signal.as_ref(),
        )
        .await?;
        let Some(definition) = definition else {
            return Ok(None);
        };
        validate_definition(&definition)?;
        if definition.name != entry.candidate.name {
            self.invalidate_entry(entry);
            return Ok(None);
        }
        Ok(Some(definition))
    }

    async fn collect(&self, options: &SkillViewOptions) -> Result<CollectResult, String> {
        throw_if_aborted(options.signal.as_ref())?;
        let mut attempt = 1;
        loop {
            let revision = self.revision.load(Ordering::SeqCst);
            let key = self.collect_cache_key(options, revision);
            if let Some(cached) = self.collect_cache.lock().get(&key).cloned() {
                return Ok(CollectResult {
                    entries: cached,
                    cacheable: true,
                });
            }
            let result = self.collect_fresh(options).await?;
            throw_if_aborted(options.signal.as_ref())?;
            if revision != self.revision.load(Ordering::SeqCst) {
                if attempt < MAX_COLLECT_ATTEMPTS {
                    attempt += 1;
                    continue;
                }
                return Ok(CollectResult {
                    entries: result.entries,
                    cacheable: false,
                });
            }
            if result.cacheable {
                let mut cache = self.collect_cache.lock();
                cache.insert(key, result.entries.clone());
                if cache.len() > self.collect_cache_max_entries
                    && let Some(oldest) = cache.keys().next().cloned()
                {
                    cache.shift_remove(&oldest);
                }
            }
            return Ok(result);
        }
    }

    async fn collect_fresh(&self, options: &SkillViewOptions) -> Result<CollectResult, String> {
        // Global first, then existing chain overlays farthest ancestor
        // first and the exact scope last, so the nearest layer's same-name
        // entry replaces the farther ones.
        let mut layers = vec![self.layers.global.clone()];
        layers.extend(self.layers.chain_layers(options.scope.as_ref()));
        let mut merged: IndexMap<String, IndexedCandidate> = IndexMap::new();
        let mut cacheable = true;
        for layer in &layers {
            let collected = self.collect_layer(layer, options).await?;
            if !collected.cacheable {
                cacheable = false;
            }
            for entry in collected.entries {
                merged.insert(entry.candidate.name.clone(), entry);
            }
        }
        Ok(CollectResult {
            entries: merged,
            cacheable,
        })
    }

    async fn collect_layer(
        &self,
        layer: &Arc<SkillLayer>,
        options: &SkillViewOptions,
    ) -> Result<LayerCollectResult, String> {
        let collected = self.list_layer_candidates(layer, options).await?;
        let mut entries = collected.entries;
        entries.sort_by(compare_indexed_candidates);
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for entry in entries {
            let name = entry.candidate.name.clone();
            if seen.contains(&name) {
                self.ctx.named_logger(None).warn(vec![arc(format!(
                    "skill \"{name}\" from {} ignored because a higher-priority skill already exists",
                    entry.candidate.source
                ))]);
                continue;
            }
            seen.insert(name);
            result.push(entry);
        }
        Ok(LayerCollectResult {
            entries: result,
            cacheable: collected.cacheable,
        })
    }

    async fn list_layer_candidates(
        &self,
        layer: &Arc<SkillLayer>,
        options: &SkillViewOptions,
    ) -> Result<LayerCollectResult, String> {
        throw_if_aborted(options.signal.as_ref())?;
        let mut candidates: Vec<IndexedCandidate> = Vec::new();
        let mut cacheable = true;
        let mut runtime_order = 0;
        let runtime = layer.runtime.lock().clone();
        let mut runtime_skills: Vec<SkillDefinition> = runtime.values().cloned().collect();
        runtime_skills.sort_by(|left, right| compare_code_points(&left.name, &right.name));
        for skill in runtime_skills {
            candidates.push(IndexedCandidate {
                candidate: runtime_candidate(&skill),
                provider: runtime_provider(),
                provider_order: -1,
                local_order: runtime_order,
                layer: layer.clone(),
            });
            runtime_order += 1;
        }
        for (provider_name, registered) in layer.providers.entries() {
            let mut local_order = 0;
            let observation = match wait_with_abort(
                registered.provider.list(&options.lookup()),
                options.signal.as_ref(),
            )
            .await
            {
                Ok(observation) => observation,
                Err(error) => {
                    if options.signal.as_ref().is_some_and(|signal| signal()) {
                        return Err(error);
                    }
                    cacheable = false;
                    self.ctx.named_logger(None).warn(vec![arc(format!(
                        "skill provider \"{provider_name}\" skipped: {error}"
                    ))]);
                    continue;
                }
            };
            if !observation.complete {
                cacheable = false;
            }
            for candidate in observation.candidates {
                validate_candidate(&candidate, &provider_name)?;
                candidates.push(IndexedCandidate {
                    candidate,
                    provider: registered.provider.clone(),
                    provider_order: registered.order as i64,
                    local_order,
                    layer: layer.clone(),
                });
                local_order += 1;
            }
        }
        Ok(LayerCollectResult {
            entries: candidates,
            cacheable,
        })
    }

    fn invalidate_cache(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        self.collect_cache.lock().clear();
        self.notify_change();
    }

    /// Invalidate after a stale definition load, only while the exact
    /// registration that produced the entry is still live.
    fn invalidate_entry(&self, entry: &IndexedCandidate) {
        let is_live = entry
            .layer
            .providers
            .get(entry.provider.name())
            .is_some_and(|registered| Arc::ptr_eq(&registered.provider, &entry.provider));
        if is_live {
            self.invalidate_cache();
        }
    }

    fn collect_cache_key(&self, options: &SkillViewOptions, revision: u64) -> String {
        let chain = scope_chain_of(options.scope.as_ref());
        let mut scope_ids = self.scope_ids.lock();
        let ids: Vec<u64> = chain
            .iter()
            .map(|key| {
                if let Some(id) = scope_ids.get(key) {
                    *id
                } else {
                    let id = self.next_scope_id.fetch_add(1, Ordering::Relaxed);
                    scope_ids.insert(key.clone(), id);
                    id
                }
            })
            .collect();
        serde_json::to_string(&serde_json::json!({
            "cwd": options.cwd,
            "scopes": ids,
            "revision": revision,
        }))
        .expect("cache key serializes")
    }

    /// Notify catalog observers without making their refresh work
    /// load-bearing. Listener failures are contained and cannot veto the
    /// registry mutation.
    fn notify_change(&self) {
        let listeners = self.ctx.collect(DispatchMode::Emit, "skills/change", &[]);
        for (_ctx, callback) in listeners {
            // The synchronous callback invocation and its future polling both
            // ride the containment (the TS catch wraps the whole dispatch).
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let future = callback(&self.ctx, Vec::new());
                futures::executor::block_on(future)
            }));
            if outcome.is_err() {
                self.ctx
                    .named_logger(None)
                    .warn(vec![arc("skills/change listener threw".to_string())]);
            }
        }
    }
}

fn runtime_candidate(skill: &SkillDefinition) -> SkillCandidate {
    SkillCandidate {
        name: skill.name.clone(),
        description: skill.description.clone(),
        when_to_use: skill.when_to_use.clone(),
        invocation: skill.invocation,
        source: skill.source.clone(),
        provider: skill.provider.clone(),
        resource_base: skill.resource_base.clone(),
        rank: RUNTIME_RANK,
        locator: arc(skill.clone()),
        path: skill.path.clone(),
        metadata: skill.metadata.clone(),
    }
}

fn validate_candidate(candidate: &SkillCandidate, provider_name: &str) -> Result<(), String> {
    if !is_skill_name(&candidate.name) {
        return Err(format!(
            "skill provider \"{provider_name}\" returned invalid skill name \"{}\"",
            candidate.name
        ));
    }
    if candidate.description.is_empty() {
        return Err(format!(
            "skill provider \"{provider_name}\" returned skill \"{}\" without a description",
            candidate.name
        ));
    }
    if candidate.provider != provider_name {
        return Err(format!(
            "skill provider \"{provider_name}\" returned skill \"{}\" for provider \"{}\"",
            candidate.name, candidate.provider
        ));
    }
    Ok(())
}

fn validate_runtime_skill(skill: &SkillRegistration) {
    if !is_skill_name(&skill.name) {
        panic!("invalid skill name \"{}\"", skill.name);
    }
    if skill.description.is_empty() {
        panic!("skill \"{}\" requires a description", skill.name);
    }
}

/// Validate a definition loaded from a provider-controlled parser or
/// remote source.
fn validate_definition(skill: &SkillDefinition) -> Result<(), String> {
    if !is_skill_name(&skill.name) {
        return Err(format!("loaded skill has invalid name \"{}\"", skill.name));
    }
    if skill.description.is_empty() {
        return Err(format!(
            "loaded skill \"{}\" requires a description",
            skill.name
        ));
    }
    Ok(())
}

fn to_summary(skill: &SkillCandidate) -> SkillSummary {
    SkillSummary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        when_to_use: skill.when_to_use.clone(),
        invocation: skill.invocation,
        source: skill.source.clone(),
        provider: skill.provider.clone(),
        resource_base: skill.resource_base.clone(),
    }
}

fn compare_code_points(left: &str, right: &str) -> std::cmp::Ordering {
    left.cmp(right)
}

fn compare_indexed_candidates(
    left: &IndexedCandidate,
    right: &IndexedCandidate,
) -> std::cmp::Ordering {
    left.candidate
        .rank
        .cmp(&right.candidate.rank)
        .then(left.provider_order.cmp(&right.provider_order))
        .then(left.local_order.cmp(&right.local_order))
}

fn throw_if_aborted(signal: Option<&SkillAbort>) -> Result<(), String> {
    if signal.is_some_and(|signal| signal()) {
        return Err(SKILL_ABORTED_MESSAGE.to_string());
    }
    Ok(())
}

async fn wait_with_abort<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T, String>> + Send,
    signal: Option<&SkillAbort>,
) -> Result<T, String> {
    let Some(signal) = signal else {
        return future.await;
    };
    if signal() {
        return Err(SKILL_ABORTED_MESSAGE.to_string());
    }
    tokio::pin!(future);
    let poller = {
        let signal = signal.clone();
        async move {
            loop {
                if signal() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    };
    tokio::pin!(poller);
    tokio::select! {
        result = &mut future => result,
        _ = &mut poller => Err(SKILL_ABORTED_MESSAGE.to_string()),
    }
}

/// The Cordis plugin form (TS mounts the service class with the schema).
pub struct SkillPlugin {
    config: Config,
}

impl SkillPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for SkillPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("skill")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        SkillRegistry::install(ctx, self.config.clone())
            .map(|_| ())
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
