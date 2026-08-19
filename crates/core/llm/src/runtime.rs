//! LLM runtime service: adapter registry with a waterfall-interceptable
//! streaming call API. Rust port of `packages/llm/llm/src/index.ts`
//! (runtime half).
//!
//! # Deviations
//!
//! - The `llm/stream` waterfall returns an async chunk iterable in TS; the
//!   Rust waterfall carries a [`StreamFactory`] instead, the request rides
//!   a shared cell so listener mutations reach the adapter (the TS in-place
//!   writes), and [`LlmRuntime::stream`] resolves the factory on a dedicated
//!   thread (the synchronous-dispatch pattern shared with the telemetry
//!   coordinator).
//! - `AbortSignal` collapses to a cancellation predicate on
//!   [`GenerateOptions::signal`].
//! - `markAgentLoopRequest`/`isAgentLoopRequest` ride an explicit flag on
//!   the request object (see `call-config.rs`).
//! - Adapter failures cross the adapter boundary as strings; panics from
//!   adapter dispatch or iteration are rendered and normalized with the
//!   `UNKNOWN` code (see `adapter-failure.rs`).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{
    ArcValue, BoxFuture, Context, DispatchMode, Disposer, Service, arc, downcast_arc, make_disposer,
};
use futures::{FutureExt, StreamExt};
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::adapter_failure::normalize_llm_failure;
use crate::api_key::{ApiKeyCheck, ApiKeyRejection, normalize_api_key};
use crate::call_config::call_config_equals;
use crate::error::INVALID_CREDENTIAL_CODE;
use crate::message::{Message, MessageSource, Role};
use crate::retry_policy::{ResolvedRetryPolicy, resolve_retry_policy};
use crate::types::{
    FinishReason, GenerateOptions, LlmCallConfig, LlmCallConfigAdapterDefaults,
    LlmConfigurableProvider, LlmDiscoveredModel, LlmFailure, LlmModelContext,
    LlmModelDiscoveryRequest, LlmModelInfo, LlmProviderInfo, LlmResolvedModelInfo, StreamChunk,
};

/// A chunk stream (the TS `AsyncIterable<StreamChunk>`).
pub type ChunkStream = futures::stream::BoxStream<'static, StreamChunk>;

/// A stream factory: `GenerateOptions → ChunkStream` (the `llm/stream`
/// waterfall payload).
pub type StreamFactory = Arc<dyn Fn(GenerateOptions) -> ChunkStream + Send + Sync>;

/// Cancellation predicate (TS `AbortSignal`).
type AbortSignal = Arc<dyn Fn() -> bool + Send + Sync>;

/// Structured provider facts and cause accepted by [`LlmError`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlmErrorOptions {
    /// Valid HTTP status observed at the provider boundary.
    pub status: Option<u64>,
    /// Positive finite provider-requested delay in milliseconds.
    pub provider_retry_after_ms: Option<u64>,
    /// Non-empty opaque provider request id.
    pub request_id: Option<crate::brand::ProviderRequestId>,
}

/// Typed error for LLM-related failures.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmError {
    /// Non-empty human-readable failure summary.
    pub message: String,
    /// Non-empty stable provider-neutral machine code.
    pub code: &'static str,
    /// Serializable facts retained beside this live error.
    pub failure: LlmFailure,
}

impl LlmError {
    /// Build a validated [`LlmError`] (the TS constructor checks).
    pub fn new(message: &str, code: &'static str, options: LlmErrorOptions) -> Self {
        if message.is_empty() {
            panic!("LlmError message must be a non-empty string");
        }
        if code.is_empty() {
            panic!("LlmError code must be a non-empty string");
        }
        if options
            .status
            .is_some_and(|status| !(100..=599).contains(&status))
        {
            panic!("LlmError status must be an integer from 100 through 599");
        }
        if options
            .provider_retry_after_ms
            .is_some_and(|delay| delay == 0)
        {
            panic!("LlmError providerRetryAfterMs must be a positive finite number");
        }
        if options
            .request_id
            .as_ref()
            .is_some_and(|id| id.as_str().is_empty())
        {
            panic!("LlmError requestId must be a non-empty string");
        }
        Self {
            message: message.to_string(),
            code,
            failure: LlmFailure {
                message: message.to_string(),
                code: code.to_string(),
                status: options.status,
                provider_retry_after_ms: options.provider_retry_after_ms,
                request_id: options.request_id,
            },
        }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "LlmError: {}", self.message)
    }
}

impl std::error::Error for LlmError {}

/// Accept one supplied credential, or refuse it as unusable (TS
/// `assertUsableApiKey`).
pub fn assert_usable_api_key(raw: &str, pkg: &str, reference: &str) -> Result<String, LlmError> {
    match normalize_api_key(raw) {
        ApiKeyCheck::Ok { value } => Ok(value),
        ApiKeyCheck::Rejected { reason } => {
            let message = match reason {
                ApiKeyRejection::Empty => format!(
                    "{pkg}: the API key resolved from {reference} is blank; set {reference} to the raw key (the web Models page writes it) or export it in the launching environment"
                ),
                ApiKeyRejection::IllegalCharacters => format!(
                    "{pkg}: the API key resolved from {reference} contains characters no HTTP header can carry; set {reference} to the raw key alone (the web Models page writes it)"
                ),
            };
            Err(LlmError::new(
                &message,
                INVALID_CREDENTIAL_CODE,
                LlmErrorOptions::default(),
            ))
        }
    }
}

/// One model call whose config and adapter registration were resolved
/// together.
pub struct PreparedLlmCall {
    /// Detached, deep-frozen config with any adapter-owned default
    /// materialized.
    pub config: LlmCallConfig,
    /// Immutable retry policy captured with the adapter registration.
    pub retry_policy: ResolvedRetryPolicy,
    /// Detached context metadata resolved with the registration-bound call.
    pub context: Option<LlmModelContext>,
    /// Config fields materialized by the captured adapter rather than
    /// proposed by the caller.
    pub adapter_defaults: LlmCallConfigAdapterDefaults,
    /// Dispatch this call once through the registration captured during
    /// preparation. The request's call-config fields must match [`config`];
    /// reuse or mismatch fails with `INVALID_PREPARED_CALL`.
    ///
    /// [`config`]: PreparedLlmCall::config
    pub stream: Arc<dyn Fn(GenerateOptions) -> Result<ChunkStream, LlmError> + Send + Sync>,
}

/// Provider-wire adapter for the harness message and stream vocabulary.
#[async_trait::async_trait]
pub trait LlmAdapter: Send + Sync {
    /// Describe one provider route owned by this adapter.
    fn provider_info(&self, provider: &str) -> LlmProviderInfo {
        LlmProviderInfo {
            id: provider.to_string(),
            name: provider.to_string(),
        }
    }

    /// Return the provider-owned retry policy captured with this route.
    fn provider_retry_policy(&self, _provider: &str) -> Option<ResolvedRetryPolicy> {
        None
    }

    /// List models this adapter can currently advertise for one owned
    /// provider. The result is advisory.
    async fn list_models(&self, _provider: &str) -> Vec<LlmModelInfo> {
        Vec::new()
    }

    /// Resolve all metadata available for one exact model. This query is
    /// independent of the advisory catalog and does not validate request
    /// routing.
    async fn resolve_model(
        &self,
        provider: &str,
        model: &str,
        _signal: Option<&AbortSignal>,
    ) -> LlmResolvedModelInfo {
        LlmResolvedModelInfo {
            provider: provider.to_string(),
            id: model.to_string(),
            name: model.to_string(),
            description: None,
            input_modalities: None,
            context: None,
            default_max_tokens: None,
            reasoning: None,
        }
    }

    /// Stream one model call as raw chunks. The only required method.
    fn stream(&self, options: &GenerateOptions) -> ChunkStream;
}

/// What [`LlmRuntime::register_adapter`] returns: the disposer, plus an
/// atomic route replacement for the same adapter instance.
pub struct AdapterRegistrationHandle {
    /// Release every route this registration currently holds.
    pub dispose: Disposer,
    /// Replace this registration's routes with `providers`, keeping the
    /// same adapter instance.
    pub replace: Arc<dyn Fn(Vec<String>) -> Result<(), LlmError> + Send + Sync>,
}

/// A live configurable-provider registration, disposable and atomically
/// replaceable — the directory counterpart of
/// [`AdapterRegistrationHandle`].
pub struct DirectoryRegistrationHandle {
    /// Withdraw every entry this registration currently holds.
    pub dispose: Disposer,
    /// Replace this registration's entries with `entries`.
    pub replace: Arc<dyn Fn(Vec<LlmConfigurableProvider>) -> Result<(), LlmError> + Send + Sync>,
}

struct AdapterRegistration {
    adapter: Arc<dyn LlmAdapter>,
    provider: LlmProviderInfo,
    retry_policy: ResolvedRetryPolicy,
}

type DiscoveryFn = Arc<
    dyn Fn(&LlmModelDiscoveryRequest) -> BoxFuture<'static, Result<Vec<LlmDiscoveredModel>, String>>
        + Send
        + Sync,
>;

/// The abstract `llm` service: an adapter registry plus a streaming
/// model-call API, interceptable via the `llm/stream` waterfall.
pub struct LlmRuntime {
    ctx: Context,
    adapters: Mutex<IndexMap<String, Arc<AdapterRegistration>>>,
    directory: Mutex<IndexMap<String, LlmConfigurableProvider>>,
    discoveries: Mutex<IndexMap<String, DiscoveryFn>>,
}

impl Service for LlmRuntime {
    fn service_name(&self) -> &'static str {
        "llm"
    }
}

impl LlmRuntime {
    /// Create the runtime and register it as `ctx.llm`.
    pub fn install(ctx: &Context) -> Arc<Self> {
        let runtime = Arc::new(Self {
            ctx: ctx.clone(),
            adapters: Mutex::new(IndexMap::new()),
            directory: Mutex::new(IndexMap::new()),
            discoveries: Mutex::new(IndexMap::new()),
        });
        ctx.register_service(runtime.clone());
        runtime
    }

    /// Notify topology observers without letting one broken listener veto
    /// the commit. `INVARIANT`-coded failures still surface.
    fn emit_adapters_updated(&self) {
        let args: Vec<ArcValue> = Vec::new();
        let listeners = self
            .ctx
            .collect(DispatchMode::Emit, "llm/adapters-updated", &args);
        let mut invariant_failure: Option<String> = None;
        for (listener_ctx, listener) in listeners {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                futures::executor::block_on(listener(&listener_ctx, args.clone()));
            }));
            match outcome {
                Ok(_) => {}
                Err(payload) => {
                    if is_invariant_failure(&payload) {
                        if invariant_failure.is_none() {
                            invariant_failure = Some(render_panic(payload));
                        }
                        continue;
                    }
                    self.warn_adapters_listener_failure(render_panic(payload));
                }
            }
        }
        if let Some(failure) = invariant_failure {
            panic!("{failure}");
        }
    }

    /// Contained-listener diagnostic shared by the failure paths.
    fn warn_adapters_listener_failure(&self, error: String) {
        self.ctx.named_logger(Some("llm")).warn(vec![arc(format!(
            "an llm/adapters-updated listener failed: {error}"
        ))]);
    }

    /// Register an adapter for the given provider routes. Throws `LlmError`
    /// with code `DUPLICATE_ADAPTER` if any provider already has an adapter
    /// (all-or-nothing). Disposed with the fiber.
    pub fn register_adapter(
        self: &Arc<Self>,
        caller: &Context,
        providers: Vec<String>,
        adapter: Arc<dyn LlmAdapter>,
    ) -> Result<AdapterRegistrationHandle, LlmError> {
        // The routes this registration currently holds; `replace` rewrites
        // it, and the disposer releases whatever it holds at disposal time.
        let owned: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        // The disposer has run: `owned` being empty cannot say so on its
        // own, because `replace([])` legally leaves a live registration
        // holding none.
        let released: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        {
            if providers.is_empty() {
                return Err(LlmError::new(
                    "an adapter must register at least one provider",
                    "INVALID_ADAPTER",
                    LlmErrorOptions::default(),
                ));
            }
            let registrations = self.prepare_routes(&providers, &adapter, &owned.lock())?;
            self.commit_routes(&mut owned.lock(), &registrations);
        }
        let runtime = Arc::clone(self);
        let owned_for_dispose = Arc::clone(&owned);
        let released_for_dispose = Arc::clone(&released);
        let dispose = caller.effect(
            "llm.registerAdapter()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = Arc::clone(&runtime);
                    let owned = Arc::clone(&owned_for_dispose);
                    let released = Arc::clone(&released_for_dispose);
                    Box::pin(async move {
                        released.store(true, Ordering::SeqCst);
                        let owned = std::mem::take(&mut *owned.lock());
                        let mut adapters = runtime.adapters.lock();
                        for provider in owned {
                            adapters.shift_remove(&provider);
                        }
                        drop(adapters);
                        runtime.emit_adapters_updated();
                    })
                }))
            }),
        );
        let replace_runtime = Arc::clone(self);
        let replace_owned = Arc::clone(&owned);
        let replace_released = Arc::clone(&released);
        let replace_adapter = Arc::clone(&adapter);
        let replace = Arc::new(move |next: Vec<String>| {
            if replace_released.load(Ordering::SeqCst) {
                return Err(LlmError::new(
                    "a disposed adapter registration cannot replace its routes",
                    "REGISTRATION_DISPOSED",
                    LlmErrorOptions::default(),
                ));
            }
            let registrations =
                replace_runtime.prepare_routes(&next, &replace_adapter, &replace_owned.lock())?;
            replace_runtime.commit_routes(&mut replace_owned.lock(), &registrations);
            Ok(())
        });
        Ok(AdapterRegistrationHandle { dispose, replace })
    }

    /// Validate one candidate route set for `adapter`, treating routes this
    /// registration already holds as available. Nothing is mutated: a
    /// rejected candidate leaves the registry exactly as it was.
    fn prepare_routes(
        &self,
        providers: &[String],
        adapter: &Arc<dyn LlmAdapter>,
        owned: &[String],
    ) -> Result<Vec<Arc<AdapterRegistration>>, LlmError> {
        let adapters = self.adapters.lock();
        let mut unique = std::collections::HashSet::new();
        let mut registrations = Vec::new();
        for provider in providers {
            if provider.is_empty() {
                return Err(LlmError::new(
                    "adapter provider names must be non-empty",
                    "INVALID_ADAPTER",
                    LlmErrorOptions::default(),
                ));
            }
            if !unique.insert(provider.clone())
                || (adapters.contains_key(provider) && !owned.contains(provider))
            {
                return Err(LlmError::new(
                    &format!("an adapter for provider \"{provider}\" is already registered"),
                    "DUPLICATE_ADAPTER",
                    LlmErrorOptions::default(),
                ));
            }
            let info = adapter.provider_info(provider);
            if info.id != *provider || info.name.is_empty() {
                return Err(LlmError::new(
                    &format!(
                        "adapter metadata for provider \"{provider}\" must preserve its id and have a non-empty name"
                    ),
                    "INVALID_ADAPTER",
                    LlmErrorOptions::default(),
                ));
            }
            let retry_policy = adapter.provider_retry_policy(provider).unwrap_or_else(|| {
                resolve_retry_policy(None, &format!("llm: provider \"{provider}\" retryPolicy"))
                    .expect("defaults always resolve")
            });
            registrations.push(Arc::new(AdapterRegistration {
                adapter: Arc::clone(adapter),
                provider: info,
                retry_policy,
            }));
        }
        Ok(registrations)
    }

    /// Swap this registration's routes for the prepared ones in one
    /// synchronous section. The route set's one mutation point is also where
    /// `llm/adapters-updated` is published.
    fn commit_routes(&self, owned: &mut Vec<String>, registrations: &[Arc<AdapterRegistration>]) {
        let mut adapters = self.adapters.lock();
        for provider in owned.iter() {
            adapters.shift_remove(provider);
        }
        owned.clear();
        for registration in registrations {
            adapters.insert(registration.provider.id.clone(), Arc::clone(registration));
            owned.push(registration.provider.id.clone());
        }
        drop(adapters);
        self.emit_adapters_updated();
    }

    /// Describe provider routes with a registered adapter.
    pub fn list_providers(&self) -> Vec<LlmProviderInfo> {
        self.adapters
            .lock()
            .values()
            .map(|registration| registration.provider.clone())
            .collect()
    }

    /// Declare provider routes an adapter plugin can activate through
    /// configuration. Registration is all-or-nothing. Disposed with the
    /// fiber.
    pub fn register_configurable_providers(
        self: &Arc<Self>,
        caller: &Context,
        entries: Vec<LlmConfigurableProvider>,
    ) -> Result<DirectoryRegistrationHandle, LlmError> {
        let held: Arc<Mutex<Vec<LlmConfigurableProvider>>> = Arc::new(Mutex::new(Vec::new()));
        let disposed: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        {
            if entries.is_empty() {
                return Err(LlmError::new(
                    "a configurable-provider registration must declare at least one provider",
                    "INVALID_DIRECTORY",
                    LlmErrorOptions::default(),
                ));
            }
            self.commit_directory(&mut held.lock(), &entries)?;
        }
        let runtime = Arc::clone(self);
        let held_for_dispose = Arc::clone(&held);
        let disposed_for_dispose = Arc::clone(&disposed);
        let dispose = caller.effect(
            "llm.registerConfigurableProviders()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = Arc::clone(&runtime);
                    let held = Arc::clone(&held_for_dispose);
                    let disposed = Arc::clone(&disposed_for_dispose);
                    Box::pin(async move {
                        disposed.store(true, Ordering::SeqCst);
                        let held = std::mem::take(&mut *held.lock());
                        let mut directory = runtime.directory.lock();
                        for entry in held {
                            directory.shift_remove(&entry.provider);
                        }
                        drop(directory);
                        runtime.emit_adapters_updated();
                    })
                }))
            }),
        );
        let replace_runtime = Arc::clone(self);
        let replace_held = Arc::clone(&held);
        let replace_disposed = Arc::clone(&disposed);
        let replace = Arc::new(move |next: Vec<LlmConfigurableProvider>| {
            if replace_disposed.load(Ordering::SeqCst) {
                return Err(LlmError::new(
                    "this configurable-provider registration was disposed",
                    "REGISTRATION_DISPOSED",
                    LlmErrorOptions::default(),
                ));
            }
            replace_runtime.commit_directory(&mut replace_held.lock(), &next)
        });
        Ok(DirectoryRegistrationHandle { dispose, replace })
    }

    /// Validate a candidate set in full against everything this registration
    /// does not already hold, then publish it. A refused candidate leaves the
    /// current entries in place.
    fn commit_directory(
        &self,
        held: &mut Vec<LlmConfigurableProvider>,
        candidates: &[LlmConfigurableProvider],
    ) -> Result<(), LlmError> {
        let own: std::collections::HashSet<String> =
            held.iter().map(|entry| entry.provider.clone()).collect();
        let directory = self.directory.lock();
        let mut detached: Vec<LlmConfigurableProvider> = Vec::new();
        for entry in candidates {
            if entry.provider.is_empty()
                || entry.display_name.is_empty()
                || entry.settings_ns.is_empty()
            {
                return Err(LlmError::new(
                    "configurable providers need a non-empty provider, displayName, and settingsNs",
                    "INVALID_DIRECTORY",
                    LlmErrorOptions::default(),
                ));
            }
            if entry.settings_path.iter().any(|segment| segment.is_empty()) {
                return Err(LlmError::new(
                    &format!(
                        "configurable provider \"{}\" has an empty settingsPath segment",
                        entry.provider
                    ),
                    "INVALID_DIRECTORY",
                    LlmErrorOptions::default(),
                ));
            }
            if (directory.contains_key(&entry.provider) && !own.contains(&entry.provider))
                || detached
                    .iter()
                    .any(|seen: &LlmConfigurableProvider| seen.provider == entry.provider)
            {
                return Err(LlmError::new(
                    &format!(
                        "configurable provider \"{}\" is already declared",
                        entry.provider
                    ),
                    "DUPLICATE_DIRECTORY",
                    LlmErrorOptions::default(),
                ));
            }
            detached.push(entry.clone());
        }
        drop(directory);
        let mut directory = self.directory.lock();
        for entry in held.iter() {
            directory.shift_remove(&entry.provider);
        }
        for entry in &detached {
            directory.insert(entry.provider.clone(), entry.clone());
        }
        drop(directory);
        *held = detached;
        self.emit_adapters_updated();
        Ok(())
    }

    /// List every declared configurable provider, registered or dormant.
    pub fn list_configurable_providers(&self) -> Vec<LlmConfigurableProvider> {
        self.directory.lock().values().cloned().collect()
    }

    /// Offer to interrogate provider endpoints on behalf of the settings
    /// namespace this plugin owns. Disposed with the fiber.
    pub fn register_model_discovery(
        self: &Arc<Self>,
        caller: &Context,
        settings_ns: &str,
        discover: DiscoveryFn,
    ) -> Result<Disposer, LlmError> {
        if settings_ns.is_empty() {
            return Err(LlmError::new(
                "model discovery needs a non-empty settings namespace",
                "INVALID_DISCOVERY",
                LlmErrorOptions::default(),
            ));
        }
        {
            let mut discoveries = self.discoveries.lock();
            if discoveries.contains_key(settings_ns) {
                return Err(LlmError::new(
                    &format!("model discovery for \"{settings_ns}\" is already registered"),
                    "DUPLICATE_DISCOVERY",
                    LlmErrorOptions::default(),
                ));
            }
            discoveries.insert(settings_ns.to_string(), discover);
        }
        let runtime = Arc::clone(self);
        let ns = settings_ns.to_string();
        Ok(caller.effect(
            "llm.registerModelDiscovery()",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let runtime = Arc::clone(&runtime);
                    let ns = ns.clone();
                    Box::pin(async move {
                        runtime.discoveries.lock().shift_remove(&ns);
                    })
                }))
            }),
        ))
    }

    /// Interrogate one provider endpoint for the models it advertises.
    pub async fn discover_models(
        &self,
        settings_ns: &str,
        request: &LlmModelDiscoveryRequest,
    ) -> Result<Vec<LlmDiscoveredModel>, LlmError> {
        let discover = self.discoveries.lock().get(settings_ns).cloned();
        let Some(discover) = discover else {
            return Err(LlmError::new(
                &format!("no model discovery is registered for \"{settings_ns}\""),
                "NO_DISCOVERY",
                LlmErrorOptions::default(),
            ));
        };
        // One of the two identifies what to describe: a route the adapter
        // knows, or an endpoint to ask. Neither leaves nothing to answer
        // about.
        if request.provider.as_deref().unwrap_or("").is_empty()
            && request.base_url.as_deref().unwrap_or("").is_empty()
        {
            return Err(LlmError::new(
                "model discovery needs a provider route or a baseURL",
                "INVALID_DISCOVERY",
                LlmErrorOptions::default(),
            ));
        }
        let discovered = discover(request)
            .await
            .map_err(|error| LlmError::new(&error, "UNKNOWN", LlmErrorOptions::default()))?;
        let mut seen = std::collections::HashSet::new();
        let mut models = Vec::new();
        for model in discovered {
            if model.id.is_empty() || !seen.insert(model.id.clone()) {
                continue;
            }
            models.push(model);
        }
        Ok(models)
    }

    /// Resolve the retry policy captured when one provider route was
    /// registered.
    pub fn provider_retry_policy(&self, provider: &str) -> Result<ResolvedRetryPolicy, LlmError> {
        Ok(self.registration(provider)?.retry_policy.clone())
    }

    /// Discover models advertised by one registered provider. Catalog
    /// membership is advisory and never changes routing or request
    /// validation.
    pub async fn list_models(&self, provider: &str) -> Result<Vec<LlmModelInfo>, LlmError> {
        let registration = self.registration(provider)?;
        let models = registration.adapter.list_models(provider).await;
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for model in models {
            if model.provider != provider
                || model.id.is_empty()
                || model.name.is_empty()
                || !seen.insert(model.id.clone())
            {
                return Err(LlmError::new(
                    &format!(
                        "adapter returned invalid or duplicate model metadata for provider \"{provider}\""
                    ),
                    "INVALID_CATALOG",
                    LlmErrorOptions::default(),
                ));
            }
            result.push(model);
        }
        Ok(result)
    }

    /// Resolve and validate all metadata from the adapter that owns one
    /// exact route.
    pub async fn resolve_model_info(
        &self,
        provider: &str,
        model: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<LlmResolvedModelInfo, LlmError> {
        let registration = self.registration(provider)?;
        Self::resolve_model_info_for(&registration, model, signal).await
    }

    async fn resolve_model_info_for(
        registration: &AdapterRegistration,
        model: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<LlmResolvedModelInfo, LlmError> {
        let provider = &registration.provider.id;
        let resolved = registration
            .adapter
            .resolve_model(provider, model, signal)
            .await;
        if resolved.provider != *provider || resolved.id != model || resolved.name.is_empty() {
            return Err(LlmError::new(
                &format!(
                    "adapter returned invalid exact model metadata for provider \"{provider}\" model \"{model}\""
                ),
                "INVALID_MODEL_INFO",
                LlmErrorOptions::default(),
            ));
        }
        if let Some(context) = &resolved.context {
            if context.context_window == 0 {
                return Err(LlmError::new(
                    &format!(
                        "adapter returned invalid context metadata for provider \"{provider}\" model \"{model}\""
                    ),
                    "INVALID_MODEL_CONTEXT",
                    LlmErrorOptions::default(),
                ));
            }
        }
        if resolved
            .default_max_tokens
            .is_some_and(|tokens| tokens == 0)
        {
            return Err(LlmError::new(
                &format!(
                    "adapter returned invalid default maxTokens for provider \"{provider}\" model \"{model}\""
                ),
                "INVALID_MODEL_MAX_TOKENS",
                LlmErrorOptions::default(),
            ));
        }
        if let Some(reasoning) = &resolved.reasoning {
            if reasoning.efforts.is_empty() {
                return Err(LlmError::new(
                    &format!(
                        "adapter returned invalid reasoning metadata for provider \"{provider}\" model \"{model}\""
                    ),
                    "INVALID_MODEL_REASONING",
                    LlmErrorOptions::default(),
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for effort in &reasoning.efforts {
                if effort.id.as_str().is_empty()
                    || effort.name.is_empty()
                    || !seen.insert(effort.id.as_str().to_string())
                {
                    return Err(LlmError::new(
                        &format!(
                            "adapter returned invalid or duplicate reasoning effort metadata for provider \"{provider}\" model \"{model}\""
                        ),
                        "INVALID_MODEL_REASONING",
                        LlmErrorOptions::default(),
                    ));
                }
            }
            if let Some(default) = &reasoning.default_effort {
                if !seen.contains(default.as_str()) {
                    return Err(LlmError::new(
                        &format!(
                            "adapter returned an unknown default reasoning effort for provider \"{provider}\" model \"{model}\""
                        ),
                        "INVALID_MODEL_REASONING",
                        LlmErrorOptions::default(),
                    ));
                }
            }
        }
        Ok(resolved)
    }

    /// Validate a conversation call config against its exact model capability
    /// and materialize adapter-configured defaults.
    pub async fn resolve_call_config(
        &self,
        config: &LlmCallConfig,
        signal: Option<&AbortSignal>,
    ) -> Result<LlmCallConfig, LlmError> {
        let registration = self.registration(&config.provider)?;
        Ok(Self::resolve_call_for(&registration, config, signal)
            .await?
            .0)
    }

    async fn resolve_call_for(
        registration: &AdapterRegistration,
        config: &LlmCallConfig,
        signal: Option<&AbortSignal>,
    ) -> Result<(LlmCallConfig, Option<LlmModelContext>), LlmError> {
        let info = Self::resolve_model_info_for(registration, &config.model, signal).await?;
        let mut resolved = config.clone();
        if resolved.max_tokens.is_none() {
            if let Some(default) = info.default_max_tokens {
                resolved.max_tokens = Some(default);
            }
        }
        let requested = resolved.reasoning_effort.clone();
        match &info.reasoning {
            None => {
                if let Some(requested) = requested {
                    return Err(LlmError::new(
                        &format!(
                            "provider \"{}\" model \"{}\" does not support reasoning effort \"{}\"",
                            config.provider,
                            config.model,
                            requested.as_str()
                        ),
                        "UNSUPPORTED_REASONING_EFFORT",
                        LlmErrorOptions::default(),
                    ));
                }
            }
            Some(reasoning) => {
                let effective = requested
                    .clone()
                    .or_else(|| reasoning.default_effort.clone());
                if let Some(effective) = &effective {
                    if !reasoning
                        .efforts
                        .iter()
                        .any(|effort| &effort.id == effective)
                    {
                        return Err(LlmError::new(
                            &format!(
                                "provider \"{}\" model \"{}\" does not support reasoning effort \"{}\"",
                                config.provider,
                                config.model,
                                effective.as_str()
                            ),
                            "UNSUPPORTED_REASONING_EFFORT",
                            LlmErrorOptions::default(),
                        ));
                    }
                    if requested != Some(effective.clone()) {
                        resolved.reasoning_effort = Some(effective.clone());
                    }
                }
            }
        }
        Ok((resolved, info.context))
    }

    /// Resolve one call under its current adapter registration. The returned
    /// one-shot handle keeps that registration across header logging and
    /// dispatch, so HMR cannot combine one adapter's capability result with
    /// another adapter.
    pub async fn prepare_call(
        self: &Arc<Self>,
        config: &LlmCallConfig,
        signal: Option<&AbortSignal>,
    ) -> Result<PreparedLlmCall, LlmError> {
        let registration = self.registration(&config.provider)?;
        let (resolved_config, context) =
            Self::resolve_call_for(registration.as_ref(), config, signal).await?;
        let adapter_defaults = LlmCallConfigAdapterDefaults {
            reasoning_effort: (config.reasoning_effort.is_none()
                && resolved_config.reasoning_effort.is_some())
            .then_some(true),
            max_tokens: (config.max_tokens.is_none() && resolved_config.max_tokens.is_some())
                .then_some(true),
        };
        let runtime = Arc::clone(self);
        let registration_for_stream = Arc::clone(&registration);
        let resolved_for_stream = resolved_config.clone();
        let dispatched: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let stream: Arc<dyn Fn(GenerateOptions) -> Result<ChunkStream, LlmError> + Send + Sync> =
            Arc::new(move |options: GenerateOptions| {
                {
                    let mut dispatched = dispatched.lock();
                    if *dispatched {
                        return Err(LlmError::new(
                            "a prepared LLM call can only be dispatched once",
                            "INVALID_PREPARED_CALL",
                            LlmErrorOptions::default(),
                        ));
                    }
                    if !generate_options_config_equals(&options, &resolved_for_stream) {
                        return Err(LlmError::new(
                            "prepared LLM call config changed before adapter dispatch",
                            "INVALID_PREPARED_CALL",
                            LlmErrorOptions::default(),
                        ));
                    }
                    *dispatched = true;
                }
                Ok(runtime.stream_with_registration(
                    options,
                    Some((
                        Arc::clone(&registration_for_stream),
                        resolved_for_stream.clone(),
                    )),
                ))
            });
        Ok(PreparedLlmCall {
            config: resolved_config,
            retry_policy: registration.retry_policy.clone(),
            context,
            adapter_defaults,
            stream,
        })
    }

    fn registration(&self, provider: &str) -> Result<Arc<AdapterRegistration>, LlmError> {
        self.adapters.lock().get(provider).cloned().ok_or_else(|| {
            LlmError::new(
                &format!("no adapter registered for provider \"{provider}\""),
                "NO_ADAPTER",
                LlmErrorOptions::default(),
            )
        })
    }

    /// Remove replay state whose historical route is owned by another
    /// adapter.
    fn for_adapter(
        &self,
        options: GenerateOptions,
        adapter: &Arc<dyn LlmAdapter>,
    ) -> GenerateOptions {
        let adapters = self.adapters.lock();
        let mut changed = false;
        let messages: Vec<Message> = options
            .messages
            .iter()
            .map(|message| {
                let MessageSource::Model {
                    provider,
                    model,
                    replay_state: Some(_),
                } = &message.source
                else {
                    return message.clone();
                };
                if message.role != Role::Assistant {
                    return message.clone();
                }
                if adapters
                    .get(provider)
                    .is_some_and(|registration| Arc::ptr_eq(&registration.adapter, adapter))
                {
                    return message.clone();
                }
                changed = true;
                let mut stripped = message.clone();
                stripped.source = MessageSource::Model {
                    provider: provider.clone(),
                    model: model.clone(),
                    replay_state: None,
                };
                stripped
            })
            .collect();
        drop(adapters);
        if !changed {
            return options;
        }
        let mut filtered = options;
        filtered.messages = messages;
        filtered
    }

    /// Final adapter boundary. Adapter selection, dispatch, iterator
    /// construction, and iteration failures become one terminal failure
    /// chunk. Middleware and downstream consumer failures remain thrown
    /// plugin or consumer errors.
    fn adapter_stream(
        self: &Arc<Self>,
        options: GenerateOptions,
        prepared: Option<(Arc<AdapterRegistration>, LlmCallConfig)>,
    ) -> ChunkStream {
        let runtime = Arc::clone(self);
        Box::pin(futures::stream::unfold(AdapterPhase::Setup, move |phase| {
            let runtime = Arc::clone(&runtime);
            let options = options.clone();
            let prepared = prepared.clone();
            async move { runtime.adapter_phase(phase, options, prepared).await }
        }))
    }

    async fn adapter_phase(
        self: &Arc<Self>,
        phase: AdapterPhase,
        options: GenerateOptions,
        prepared: Option<(Arc<AdapterRegistration>, LlmCallConfig)>,
    ) -> Option<(StreamChunk, AdapterPhase)> {
        match phase {
            AdapterPhase::Setup => {
                let signal = options.signal.clone();
                let (registration, resolved_config) = match &prepared {
                    Some((registration, config)) => (Arc::clone(registration), config.clone()),
                    None => {
                        let registration = match self.registration(&options.provider) {
                            Ok(registration) => registration,
                            Err(error) => {
                                let chunk = adapter_failure_chunk(error.failure, signal.as_ref());
                                return Some((chunk, AdapterPhase::Done));
                            }
                        };
                        match Self::resolve_call_for(
                            registration.as_ref(),
                            &config_of(&options),
                            signal.as_ref(),
                        )
                        .await
                        {
                            Ok((config, _context)) => (registration, config),
                            Err(error) => {
                                let chunk = adapter_failure_chunk(error.failure, signal.as_ref());
                                return Some((chunk, AdapterPhase::Done));
                            }
                        }
                    }
                };
                if prepared.is_some() && !generate_options_config_equals(&options, &resolved_config)
                {
                    let failure = LlmFailure {
                        message: "prepared LLM call config changed before adapter dispatch"
                            .to_string(),
                        code: "INVALID_PREPARED_CALL".to_string(),
                        status: None,
                        provider_retry_after_ms: None,
                        request_id: None,
                    };
                    return Some((
                        adapter_failure_chunk(failure, signal.as_ref()),
                        AdapterPhase::Done,
                    ));
                }
                let resolved_options = if generate_options_config_equals(&options, &resolved_config)
                {
                    options
                } else {
                    merged_call_options(options, &resolved_config)
                };
                let adapter = Arc::clone(&registration.adapter);
                let filtered = self.for_adapter(resolved_options, &adapter);
                let mut stream = match catch_unwind(AssertUnwindSafe(|| adapter.stream(&filtered)))
                {
                    Ok(stream) => stream,
                    Err(payload) => {
                        let failure = normalize_llm_failure(&render_panic(payload));
                        return Some((
                            adapter_failure_chunk(failure, signal.as_ref()),
                            AdapterPhase::Done,
                        ));
                    }
                };
                match AssertUnwindSafe(stream.next()).catch_unwind().await {
                    Ok(Some(chunk)) => Some((chunk, AdapterPhase::Iterating(stream))),
                    Ok(None) => None,
                    Err(payload) => {
                        let failure = normalize_llm_failure(&render_panic(payload));
                        Some((
                            adapter_failure_chunk(failure, signal.as_ref()),
                            AdapterPhase::Done,
                        ))
                    }
                }
            }
            AdapterPhase::Iterating(mut stream) => {
                let signal = options.signal.clone();
                match AssertUnwindSafe(stream.next()).catch_unwind().await {
                    Ok(Some(chunk)) => Some((chunk, AdapterPhase::Iterating(stream))),
                    Ok(None) => None,
                    Err(payload) => {
                        let failure = normalize_llm_failure(&render_panic(payload));
                        Some((
                            adapter_failure_chunk(failure, signal.as_ref()),
                            AdapterPhase::Done,
                        ))
                    }
                }
            }
            AdapterPhase::Done => None,
        }
    }

    /// Stream one model call as raw chunks (token-level deltas). Replay
    /// state is retained only when the same adapter instance owns its
    /// historical provider and the target provider. Adapter selection,
    /// dispatch, and iteration failures become terminal `error` or `aborted`
    /// finish chunks; middleware, nested-call, cleanup, and consumer failures
    /// remain thrown.
    pub fn stream(self: &Arc<Self>, options: GenerateOptions) -> ChunkStream {
        self.stream_with_registration(options, None)
    }

    fn stream_with_registration(
        self: &Arc<Self>,
        options: GenerateOptions,
        prepared: Option<(Arc<AdapterRegistration>, LlmCallConfig)>,
    ) -> ChunkStream {
        let runtime = Arc::clone(self);
        // The `llm/stream` waterfall resolves a StreamFactory while the
        // request rides a shared cell, so routing listeners' mutations
        // reach the fallback adapter exactly like the TS in-place writes
        // (the factory's own argument stays a convenience snapshot).
        let cell: Arc<Mutex<GenerateOptions>> = Arc::new(Mutex::new(options.clone()));
        let fallback_factory: StreamFactory = Arc::new({
            let runtime = Arc::clone(&runtime);
            let cell = Arc::clone(&cell);
            move |_options: GenerateOptions| {
                let options = cell.lock().clone();
                runtime.adapter_stream(options, prepared.clone())
            }
        });
        let thread_runtime = Arc::clone(&runtime);
        let thread_fallback = Arc::clone(&fallback_factory);
        let thread_cell = Arc::clone(&cell);
        let factory = std::thread::spawn(move || {
            let args = vec![arc(thread_cell)];
            let fallback = Arc::clone(&thread_fallback);
            let result = futures::executor::block_on(thread_runtime.ctx.waterfall(
                "llm/stream",
                args,
                Box::pin(async move { arc(fallback) }),
            ));
            match downcast_arc::<StreamFactory>(&result) {
                Some(factory) => factory.as_ref().clone(),
                None => Arc::clone(&thread_fallback),
            }
        })
        .join()
        // A waterfall listener failure (e.g. an INVARIANT-coded guard
        // violation) must surface to the caller exactly like the TS
        // synchronous dispatch throw — replay it instead of degrading to
        // the unintercepted fallback.
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
        factory(options)
    }
}

enum AdapterPhase {
    Setup,
    Iterating(ChunkStream),
    Done,
}

/// Convert one adapter throw into the stream protocol's terminal outcome.
fn adapter_failure_chunk(failure: LlmFailure, signal: Option<&AbortSignal>) -> StreamChunk {
    let aborted = signal.is_some_and(|signal| signal());
    StreamChunk::Finish {
        reason: if aborted || failure.code == "ABORTED" {
            FinishReason::Aborted { failure }
        } else {
            FinishReason::Error { failure }
        },
        replay_state: None,
    }
}

/// The call-config projection of a [`GenerateOptions`] (the TS
/// `callConfigEquals(options, config)` comparison).
fn config_of(options: &GenerateOptions) -> LlmCallConfig {
    LlmCallConfig {
        provider: options.provider.clone(),
        model: options.model.clone(),
        reasoning_effort: options.reasoning_effort.clone(),
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        stop: options.stop.clone(),
    }
}

/// Whether a request's call-config fields match a prepared config (TS
/// `callConfigEquals(options, config)`).
pub fn generate_options_config_equals(options: &GenerateOptions, config: &LlmCallConfig) -> bool {
    call_config_equals(&config_of(options), config)
}

/// Spread a resolved call config over a request whose proposed fields the
/// resolution changed (the TS `{ ...options, ...resolvedConfig }`).
fn merged_call_options(options: GenerateOptions, config: &LlmCallConfig) -> GenerateOptions {
    let mut merged = options;
    merged.provider = config.provider.clone();
    merged.model = config.model.clone();
    merged.reasoning_effort = config.reasoning_effort.clone();
    merged.temperature = config.temperature;
    merged.max_tokens = config.max_tokens;
    merged.stop = config.stop.clone();
    merged
}

/// Whether a panic payload is an invariant failure (code `INVARIANT`).
///
/// Takes the boxed payload (rather than a `&(dyn Any + Send)` view): the
/// boxed trait object's own downcast sees the panic payload's concrete
/// type, while a re-referenced `dyn Any + Send` view can observe a
/// different `TypeId` for the same value on MSVC panic payloads.
fn is_invariant_failure(payload: &Box<dyn std::any::Any + Send>) -> bool {
    if let Some(error) = payload.downcast_ref::<dsh_invariants::InvariantError>() {
        return error.code == "INVARIANT";
    }
    // The invariants companion's `fail` channel panics with the rendered
    // error text, which carries the same prefix.
    payload
        .downcast_ref::<String>()
        .is_some_and(|message| message.starts_with("invariant violated by"))
}

/// Render a panic payload to a string.
fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return message.to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "adapter panicked".to_string()
}
