//! Service Definition for the user-settings capability seam (`ctx.settings`).
//! Rust port of `packages/settings/settings/src/index.ts`.
//!
//! # Deviations
//!
//! - `SettingsNamespace` is a branded `String` (`settings_namespace()`
//!   returns `Result` instead of throwing).
//! - `deepFreeze` has no Rust equivalent; resolved values are owned `Data`
//!   values handed out by clone, so a caller can never mutate the
//!   authoritative cell (observably equivalent).
//! - `describe().schema` carries `null`: schemastery's `toJSON` wire
//!   serialization is not ported yet.
//! - The provider's `load` → `publish` initialization runs as a spawned
//!   task gated by [`SettingsProvider::ready`] (TS completes it before the
//!   service becomes injectable).
//! - `cloneJsonShaped`'s walk is subsumed by the `serde_json::Value` input
//!   type (already JSON-compatible, no cycles); non-object write inputs
//!   still reject with the same messages.
//! - `mutate` path ops ride the same serialized queue; `applyPathOp`
//!   reproduces TS semantics including implicit intermediate creation.
#![allow(clippy::type_complexity)] // Public callback seams intentionally mirror the TS service API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{
    ArcValue, BoxFuture, Context, DispatchMode, Disposer, InjectSpec, Service, arc, make_disposer,
};
use dsh_brand::Branded;
use futures::FutureExt;
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemastery::{Data, Schema};

pub use crate::redact::{RedactedSecret, RedactedValue, redact_secrets};
pub use crate::types::{SETTINGS_DOCUMENT_UPDATED, SETTINGS_UPDATED, SettingsUpdateSource};

#[doc(hidden)]
pub enum SettingsNamespaceTag {}
/// Nominal id of one registered settings namespace (TS
/// `SettingsNamespace`).
pub type SettingsNamespace = Branded<SettingsNamespaceTag>;

/// Brand a raw string as a [`SettingsNamespace`] (lowercase kebab-case).
pub fn settings_namespace(value: &str) -> Result<SettingsNamespace, String> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(format!(
            "settings namespace \"{value}\" must match /^[a-z][a-z0-9-]*$/"
        ));
    };
    let valid = first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if !valid {
        return Err(format!(
            "settings namespace \"{value}\" must match /^[a-z][a-z0-9-]*$/"
        ));
    }
    Ok(Branded::new(value))
}

/// When a namespace's changes take effect for its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsApplies {
    Live,
    Restart,
}

impl SettingsApplies {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingsApplies::Live => "live",
            SettingsApplies::Restart => "restart",
        }
    }
}

/// Registration options beyond the namespace schema.
pub struct SettingsRegisterOptions {
    /// Composition-layer values resolved below the user layer.
    pub base: Option<Data>,
    /// Owner's effect timing (defaults to `live`).
    pub applies: SettingsApplies,
    /// Reject a resolved section the owner could not act on.
    pub validate: Option<Arc<dyn Fn(&Data) -> Result<(), String> + Send + Sync>>,
}

impl Default for SettingsRegisterOptions {
    fn default() -> Self {
        Self {
            base: None,
            applies: SettingsApplies::Live,
            validate: None,
        }
    }
}

/// One registered namespace as surfaced to configuration UIs.
#[derive(Clone)]
pub struct SettingsDescriptor {
    pub ns: SettingsNamespace,
    /// Serialized schemastery schema; `null` until `toJSON` is ported.
    pub schema: serde_json::Value,
    pub value: Data,
    pub revision: u64,
    pub base: Option<Data>,
    pub user: Option<Data>,
    pub applies: SettingsApplies,
    pub secrets: Vec<RedactedSecret>,
}

/// Options for [`SettingsProvider::describe`].
#[derive(Default)]
pub struct SettingsDescribeOptions {
    pub redact_secrets: bool,
}

/// Owner-facing handle for one registered namespace.
#[derive(Clone)]
pub struct SettingsScope {
    pub get: Arc<dyn Fn() -> Data + Send + Sync>,
    pub watch: Arc<
        dyn Fn(Arc<dyn Fn(&Data, &Data) -> BoxFuture<'static, ()> + Send + Sync>) -> Disposer
            + Send
            + Sync,
    >,
    pub update:
        Arc<dyn Fn(serde_json::Value) -> BoxFuture<'static, Result<(), String>> + Send + Sync>,
    pub replace:
        Arc<dyn Fn(serde_json::Value) -> BoxFuture<'static, Result<(), String>> + Send + Sync>,
}

/// A write refused because the namespace moved since the caller read it.
#[derive(Debug, Clone)]
pub struct SettingsConflictError {
    pub expected: u64,
    pub actual: u64,
}

impl std::fmt::Display for SettingsConflictError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "settings namespace changed since it was read (expected revision {}, now {})",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for SettingsConflictError {}

/// Deep equality over JSON-compatible data (TS `deepEqualJson`).
pub fn deep_equal_json(a: &Data, b: &Data) -> bool {
    Data::deep_equal(a, b, false)
}

/// Provider-facing storage contract (TS `load`/`persist` plus metadata).
#[async_trait::async_trait]
pub trait SettingsStorage: Send + Sync {
    /// Whether updates may persist through this provider.
    fn writable(&self) -> bool;

    /// Absolute path of the user-editable document, when file-backed.
    fn document_path(&self) -> Option<String> {
        None
    }

    /// Read the provider's current raw document.
    async fn load(&self) -> Result<IndexMap<String, Data>, String>;

    /// Durably store one namespace's merged user section.
    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String>;
}

/// One registered watcher and its serialized invocation chain.
struct Watcher {
    callback: Arc<dyn Fn(&Data, &Data) -> BoxFuture<'static, ()> + Send + Sync>,
    /// Settled tail: invocations run one at a time, in commit order.
    tail: Mutex<Option<futures::future::Shared<BoxFuture<'static, ()>>>>,
    /// Cleared by the disposer.
    active: AtomicBool,
}

/// One live namespace registration owned by a registrant fiber.
struct Registration {
    ns: SettingsNamespace,
    schema: Schema,
    base: Option<Data>,
    applies: SettingsApplies,
    validate: Option<Arc<dyn Fn(&Data) -> Result<(), String> + Send + Sync>>,
    resolved: Mutex<Data>,
    revision: Mutex<u64>,
    watchers: Mutex<Vec<Arc<Watcher>>>,
}

/// Abstract settings service. Providers implement raw-document storage and
/// push external changes through [`SettingsProvider::publish`].
pub struct SettingsProvider {
    ctx: Context,
    storage: Arc<dyn SettingsStorage>,
    registrations: Mutex<IndexMap<String, Arc<Registration>>>,
    /// Latest published raw document; empty until the first publish.
    document: Mutex<IndexMap<String, Data>>,
    /// Per-namespace write chains (settled tails).
    chains: Mutex<HashMap<String, futures::future::Shared<BoxFuture<'static, Result<(), String>>>>>,
    /// In-flight watcher invocation segments.
    pending_tails: Mutex<Vec<futures::future::Shared<BoxFuture<'static, ()>>>>,
    stopped: AtomicBool,
    ready: tokio::sync::OnceCell<Result<(), String>>,
}

impl Service for SettingsProvider {
    fn service_name(&self) -> &'static str {
        "settings"
    }
}

impl SettingsProvider {
    /// Create the provider, register the service, and start the
    /// load → publish initialization (TS `Service.init`).
    pub fn install(ctx: &Context, storage: Arc<dyn SettingsStorage>) -> Arc<Self> {
        let provider = Arc::new(Self {
            ctx: ctx.clone(),
            storage,
            registrations: Mutex::new(IndexMap::new()),
            document: Mutex::new(IndexMap::new()),
            chains: Mutex::new(HashMap::new()),
            pending_tails: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
            ready: tokio::sync::OnceCell::new(),
        });
        ctx.register_service(provider.clone());

        // Teardown: refuse new writes and watcher starts, then wait until
        // every queued write chain and started watcher invocation settles.
        let provider_for_dispose = Arc::clone(&provider);
        let _ = ctx.effect(
            "settings dispose drain",
            Box::pin(async move {
                Some(make_disposer(move || {
                    let provider = Arc::clone(&provider_for_dispose);
                    Box::pin(async move { provider.drain().await })
                }))
            }),
        );

        let provider_for_init = Arc::clone(&provider);
        tokio::spawn(async move {
            if let Err(error) = provider_for_init.initialize().await {
                provider_for_init
                    .ctx
                    .named_logger(Some("settings"))
                    .error(vec![arc(format!("initialization failed: {error}"))]);
            }
        });
        provider
    }

    /// Await the load → publish initialization.
    pub async fn ready(&self) -> Result<(), String> {
        self.ready
            .get_or_init(|| {
                let provider = self;
                async move { provider.await_init().await }
            })
            .await
            .clone()
    }

    async fn initialize(&self) -> Result<(), String> {
        self.await_init().await
    }

    async fn await_init(&self) -> Result<(), String> {
        let document = self.storage.load().await?;
        self.publish(document, SettingsUpdateSource::Provider);
        Ok(())
    }

    async fn drain(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let chains: Vec<_> = std::mem::take(&mut *self.chains.lock())
            .into_values()
            .map(|chain| chain.map(|_| ()).boxed())
            .collect();
        let tails: Vec<_> = std::mem::take(&mut *self.pending_tails.lock())
            .into_iter()
            .map(|tail| tail.map(|_| ()).boxed())
            .collect();
        let mut futures: Vec<BoxFuture<'static, ()>> = Vec::new();
        futures.extend(chains);
        futures.extend(tails);
        futures::future::join_all(futures).await;
    }

    /// Absolute path of the provider's user-editable document (TS
    /// `documentPath`).
    pub fn document_path(&self) -> Option<String> {
        self.storage.document_path()
    }

    /// Whether updates may persist through this provider (TS `writable`;
    /// the storage's own answer forwarded).
    pub fn writable(&self) -> bool {
        self.storage.writable()
    }

    /// Prepare the document for a native editor (TS `prepareDocument`).
    pub async fn prepare_document(&self) -> Option<String> {
        self.document_path()
    }

    /// Register a namespace schema and receive its owner scope (TS
    /// `register`; the registration is an effect on the CALLER fiber).
    pub fn register(
        self: &Arc<Self>,
        caller: &Context,
        ns: SettingsNamespace,
        schema: Schema,
        options: SettingsRegisterOptions,
    ) -> Result<SettingsScope, String> {
        if self.registrations.lock().contains_key(ns.as_str()) {
            return Err(format!(
                "settings namespace \"{}\" is already registered",
                ns.as_str()
            ));
        }
        let section = self.section(&ns)?;
        let resolved = self.resolve(
            &schema,
            options.base.as_ref(),
            section.as_ref(),
            options.validate.as_ref(),
        )?;
        let registration = Arc::new(Registration {
            ns: ns.clone(),
            schema,
            base: options.base,
            applies: options.applies,
            validate: options.validate,
            resolved: Mutex::new(resolved),
            revision: Mutex::new(0),
            watchers: Mutex::new(Vec::new()),
        });
        let provider = Arc::clone(self);
        let registration_for_effect = Arc::clone(&registration);
        let ns_for_effect = ns.clone();
        let _ = caller.effect(
            &format!("settings.register({})", ns.as_str()),
            Box::pin(async move {
                Some(make_disposer(move || {
                    let provider = Arc::clone(&provider);
                    let registration = Arc::clone(&registration_for_effect);
                    let ns = ns_for_effect.clone();
                    Box::pin(async move {
                        provider
                            .registrations
                            .lock()
                            .shift_remove_entry(ns.as_str());
                        let _ = registration;
                    })
                }))
            }),
        );
        self.registrations
            .lock()
            .insert(ns.as_str().to_string(), registration);
        Ok(self.scope(ns))
    }

    fn scope(self: &Arc<Self>, ns: SettingsNamespace) -> SettingsScope {
        let provider = Arc::clone(self);
        let get_ns = ns.clone();
        let watch_ns = ns.clone();
        let update_ns = ns.clone();
        let replace_ns = ns;
        let provider_get = Arc::clone(&provider);
        let provider_watch = Arc::clone(&provider);
        let provider_update = Arc::clone(&provider);
        let provider_replace = Arc::clone(&provider);
        SettingsScope {
            get: Arc::new(move || {
                provider_get
                    .registration(&get_ns)
                    .map(|registration| registration.resolved.lock().clone())
                    .unwrap_or(Data::Undefined)
            }),
            watch: Arc::new(move |callback| {
                let registration = match provider_watch.registration(&watch_ns) {
                    Some(registration) => registration,
                    None => return make_disposer(|| Box::pin(async {})),
                };
                let watcher = Arc::new(Watcher {
                    callback,
                    tail: Mutex::new(None),
                    active: AtomicBool::new(true),
                });
                registration.watchers.lock().push(Arc::clone(&watcher));
                let registration = Arc::clone(&registration);
                let watcher = Arc::clone(&watcher);
                make_disposer(move || {
                    let registration = Arc::clone(&registration);
                    let watcher = Arc::clone(&watcher);
                    Box::pin(async move {
                        watcher.active.store(false, Ordering::SeqCst);
                        registration
                            .watchers
                            .lock()
                            .retain(|entry| !Arc::ptr_eq(entry, &watcher));
                    })
                })
            }),
            update: Arc::new(move |patch| {
                let provider = Arc::clone(&provider_update);
                let ns = update_ns.clone();
                Box::pin(async move { provider.write(&ns, patch, WriteMode::Merge, None).await })
            }),
            replace: Arc::new(move |section| {
                let provider = Arc::clone(&provider_replace);
                let ns = replace_ns.clone();
                Box::pin(
                    async move { provider.write(&ns, section, WriteMode::Replace, None).await },
                )
            }),
        }
    }

    fn registration(&self, ns: &SettingsNamespace) -> Option<Arc<Registration>> {
        self.registrations.lock().get(ns.as_str()).cloned()
    }

    /// Describe every registered namespace for configuration surfaces (TS
    /// `describe`).
    pub fn describe(&self, options: SettingsDescribeOptions) -> Vec<SettingsDescriptor> {
        let registrations = self.registrations.lock();
        let document = self.document.lock();
        let mut values = Vec::new();
        registrations.values().for_each(|registration| {
            let user = self
                .section_from(&document, &registration.ns)
                .ok()
                .flatten();
            let descriptor = SettingsDescriptor {
                ns: registration.ns.clone(),
                schema: registration.schema.to_json(),
                value: registration.resolved.lock().clone(),
                revision: *registration.revision.lock(),
                base: registration.base.clone(),
                user,
                applies: registration.applies,
                secrets: Vec::new(),
            };
            if !options.redact_secrets {
                values.push(descriptor);
                return;
            }
            let redacted = redact_secrets(&registration.schema, &descriptor.value);
            values.push(SettingsDescriptor {
                value: redacted.value,
                base: descriptor
                    .base
                    .as_ref()
                    .map(|base| redact_secrets(&registration.schema, base).value),
                user: descriptor
                    .user
                    .as_ref()
                    .map(|user| redact_secrets(&registration.schema, user).value),
                secrets: redacted.secrets,
                ..descriptor
            });
        });
        values
    }

    /// Read one registered namespace's resolved value (TS `get`).
    pub fn get(&self, ns: &SettingsNamespace) -> Option<Data> {
        self.registration(ns)
            .map(|registration| registration.resolved.lock().clone())
    }

    /// Merge a patch into one namespace's user layer (TS `update`).
    pub async fn update(
        self: &Arc<Self>,
        ns: &SettingsNamespace,
        patch: serde_json::Value,
        expected_revision: Option<u64>,
    ) -> Result<(), String> {
        self.write(ns, patch, WriteMode::Merge, expected_revision)
            .await
    }

    /// Replace one namespace's user section wholesale (TS `replace`).
    pub async fn replace(
        self: &Arc<Self>,
        ns: &SettingsNamespace,
        section: serde_json::Value,
        expected_revision: Option<u64>,
    ) -> Result<(), String> {
        self.write(ns, section, WriteMode::Replace, expected_revision)
            .await
    }

    /// Apply path-addressed edits to one namespace's user section (TS
    /// `mutate`).
    pub async fn mutate(
        self: &Arc<Self>,
        ns: &SettingsNamespace,
        ops: Vec<SettingsPathOp>,
        expected_revision: Option<u64>,
    ) -> Result<(), String> {
        let payload = serde_json::json!({ "ops": ops });
        self.write(ns, payload, WriteMode::Mutate, expected_revision)
            .await
    }

    fn write(
        self: &Arc<Self>,
        ns: &SettingsNamespace,
        input: serde_json::Value,
        mode: WriteMode,
        expected_revision: Option<u64>,
    ) -> BoxFuture<'static, Result<(), String>> {
        let verb = mode.verb();
        let ns_name = ns.as_str().to_string();
        let registration = match self.registration(ns) {
            Some(registration) => registration,
            None => {
                return Box::pin(async move {
                    Err(format!(
                        "settings namespace \"{ns_name}\" is not registered"
                    ))
                });
            }
        };
        if self.stopped.load(Ordering::SeqCst) {
            let ns_name = ns_name.clone();
            return Box::pin(async move {
                Err(format!(
                    "settings service is disposed: \"{ns_name}\" cannot be written"
                ))
            });
        }
        if !self.storage.writable() {
            let ns_name = ns_name.clone();
            return Box::pin(async move {
                Err(format!(
                    "settings provider is read-only: \"{ns_name}\" cannot be updated in-process"
                ))
            });
        }
        if mode != WriteMode::Mutate && !input.is_object() {
            let ns_name = ns_name.clone();
            return Box::pin(async move {
                Err(format!(
                    "settings {verb} for \"{ns_name}\" must be a plain object"
                ))
            });
        }
        let snapshot = match data_from_json(&input) {
            Ok(snapshot) => snapshot,
            Err(label) => {
                let ns_name = ns_name.clone();
                return Box::pin(async move {
                    Err(format!(
                        "settings {verb} for \"{ns_name}\" must contain only JSON-compatible data (found {label})"
                    ))
                });
            }
        };
        let ns_owned = ns.clone();
        let provider = Arc::clone(self);
        // Serialize per namespace; a failed predecessor must not poison the
        // queue for later callers.
        let mut chains = self.chains.lock();
        let previous = chains
            .remove(ns.as_str())
            .unwrap_or_else(|| futures::future::ready(Ok(())).boxed().shared());
        let run = previous
            .then(move |_| {
                let provider = Arc::clone(&provider);
                let ns = ns_owned.clone();
                Box::pin(async move {
                    provider
                        .write_now(&ns, snapshot, mode, expected_revision, registration)
                        .await
                }) as BoxFuture<'static, Result<(), String>>
            })
            .boxed()
            .shared();
        chains.insert(ns.as_str().to_string(), run.clone());
        run.boxed()
    }

    async fn write_now(
        &self,
        ns: &SettingsNamespace,
        snapshot: Data,
        mode: WriteMode,
        expected_revision: Option<u64>,
        registration: Arc<Registration>,
    ) -> Result<(), String> {
        let verb = mode.verb();
        if self.stopped.load(Ordering::SeqCst) {
            return Err(format!(
                "settings service was disposed before the queued \"{}\" {verb} ran",
                ns.as_str()
            ));
        }
        if self.registration(ns).as_ref().map(Arc::as_ptr) != Some(Arc::as_ptr(&registration)) {
            return Err(format!(
                "settings namespace \"{}\" registration was disposed before the queued {verb} ran",
                ns.as_str()
            ));
        }
        let current = self.section(ns)?.unwrap_or(Data::Object(IndexMap::new()));
        let before_section = current.clone();
        let revision = *registration.revision.lock();
        if let Some(expected) = expected_revision
            && expected != revision
        {
            return Err(format!(
                "settings namespace \"{}\" changed since it was read (expected revision {expected}, now {revision})",
                ns.as_str()
            ));
        }
        let section = match mode {
            WriteMode::Merge => merge_layers(&current, &snapshot),
            WriteMode::Replace => snapshot,
            WriteMode::Mutate => {
                let Data::Object(payload) = &snapshot else {
                    return Err("settings mutate payload must be an object".to_string());
                };
                let ops = payload.get("ops");
                let Some(ops) = ops else {
                    return Err("settings mutate payload must carry ops".to_string());
                };
                let mut section = current;
                if let Data::Array(ops) = ops {
                    for op in ops {
                        section = apply_path_op(section, op)?;
                    }
                } else {
                    return Err(format!(
                        "settings mutate for \"{}\" ops must be an array",
                        ns.as_str()
                    ));
                }
                section
            }
        };
        let next = self.resolve(
            &registration.schema,
            registration.base.as_ref(),
            Some(&section),
            registration.validate.as_ref(),
        )?;
        self.storage.persist(ns, section.clone()).await?;
        {
            self.document
                .lock()
                .insert(ns.as_str().to_string(), section.clone());
        }
        // Commit only when this registration is still the namespace owner.
        if self.registration(ns).as_ref().map(Arc::as_ptr) == Some(Arc::as_ptr(&registration))
            && !self.stopped.load(Ordering::SeqCst)
        {
            self.bump_revision(&registration, Some(&before_section), Some(&section));
            self.commit(&registration, next, SettingsUpdateSource::Update);
        }
        Ok(())
    }

    /// Provider hook: commit a complete raw document observed in storage
    /// (TS `publish`).
    pub fn publish(&self, doc: IndexMap<String, Data>, source: SettingsUpdateSource) {
        let registrations = self.registrations.lock();
        // Read every raw section BEFORE swapping the document.
        let before: Vec<Option<Data>> = registrations
            .values()
            .map(|registration| self.section(&registration.ns).ok().flatten())
            .collect();
        *self.document.lock() = doc;
        for (registration, before_section) in registrations.values().zip(before.iter()) {
            let section = self.section(&registration.ns).ok().flatten();
            let next = match self.resolve(
                &registration.schema,
                registration.base.as_ref(),
                section.as_ref(),
                registration.validate.as_ref(),
            ) {
                Ok(next) => next,
                Err(_) => {
                    self.ctx
                        .named_logger(Some("settings"))
                        .warn(vec![arc(format!(
                            "keeping last good \"{}\" after invalid stored section",
                            registration.ns.as_str()
                        ))]);
                    continue;
                }
            };
            self.bump_revision(registration, before_section.as_ref(), section.as_ref());
            self.commit(registration, next, source);
        }
    }

    /// Read one namespace's raw user section, rejecting non-object sections.
    fn section(&self, ns: &SettingsNamespace) -> Result<Option<Data>, String> {
        self.section_from(&self.document.lock(), ns)
    }

    fn section_from(
        &self,
        document: &IndexMap<String, Data>,
        ns: &SettingsNamespace,
    ) -> Result<Option<Data>, String> {
        match document.get(ns.as_str()) {
            None => Ok(None),
            Some(Data::Object(_)) => Ok(Some(document.get(ns.as_str()).unwrap().clone())),
            Some(_) => Err(format!(
                "settings section \"{}\" must be an object of keys",
                ns.as_str()
            )),
        }
    }

    /// Resolve one namespace value: schema defaults, then `base`, then the
    /// user layer.
    fn resolve(
        &self,
        schema: &Schema,
        base: Option<&Data>,
        section: Option<&Data>,
        validate: Option<&Arc<dyn Fn(&Data) -> Result<(), String> + Send + Sync>>,
    ) -> Result<Data, String> {
        let merged = merge_layers(
            &base.cloned().unwrap_or(Data::Undefined),
            &section.cloned().unwrap_or(Data::Undefined),
        );
        let value = Schema::validate(schema, merged).map_err(|error| error.to_string())?;
        if let Some(validate) = validate {
            validate(&value)?;
        }
        Ok(value)
    }

    /// Advance a namespace's revision when its RAW section changed.
    fn bump_revision(
        &self,
        registration: &Registration,
        before: Option<&Data>,
        after: Option<&Data>,
    ) {
        let before = before.cloned().unwrap_or(Data::Undefined);
        let after = after.cloned().unwrap_or(Data::Undefined);
        if deep_equal_json(&before, &after) {
            return;
        }
        *registration.revision.lock() += 1;
        let revision = *registration.revision.lock();
        self.emit_document_updated(&registration.ns, revision);
    }

    /// Contained fan-out of `settings/document-updated`.
    fn emit_document_updated(&self, ns: &SettingsNamespace, revision: u64) {
        let args: Vec<ArcValue> = vec![arc(ns.clone()), arc(revision)];
        let listeners = self
            .ctx
            .collect(DispatchMode::Emit, SETTINGS_DOCUMENT_UPDATED, &args);
        let mut invariant_failure: Option<String> = None;
        for (listener_ctx, listener) in listeners {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                    self.warn_listener_failure(ns, render_panic(payload));
                }
            }
        }
        if let Some(failure) = invariant_failure {
            panic!("{failure}");
        }
    }

    /// Commit a resolved value when changed: swap, notify watchers, emit.
    fn commit(&self, registration: &Registration, next: Data, source: SettingsUpdateSource) {
        let prev = registration.resolved.lock().clone();
        if deep_equal_json(&next, &prev) {
            return;
        }
        *registration.resolved.lock() = next.clone();
        let watchers = registration.watchers.lock().clone();
        let ctx = self.ctx.clone();
        for watcher in watchers {
            let stopped = self.stopped.load(Ordering::SeqCst);
            let active = watcher.active.load(Ordering::SeqCst);
            let callback = Arc::clone(&watcher.callback);
            let next = next.clone();
            let prev = prev.clone();
            let ns = registration.ns.as_str().to_string();
            let ctx_for_log = ctx.clone();
            let segment = {
                let mut tail_guard = watcher.tail.lock();
                let previous = tail_guard
                    .take()
                    .unwrap_or_else(|| futures::future::ready(()).boxed().shared());
                let segment = previous
                    .then(move |_| {
                        let callback = Arc::clone(&callback);
                        let next = next.clone();
                        let prev = prev.clone();
                        let ns = ns.clone();
                        let ctx_for_log = ctx_for_log.clone();
                        Box::pin(async move {
                            if !active || stopped {
                                return;
                            }
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    futures::executor::block_on(callback(&next, &prev))
                                }));
                            if let Err(payload) = outcome {
                                ctx_for_log.named_logger(Some("settings")).warn(vec![arc(
                                    format!(
                                        "watcher for \"{ns}\" failed: {}",
                                        render_panic(payload)
                                    ),
                                )]);
                            }
                        }) as BoxFuture<'static, ()>
                    })
                    .boxed()
                    .shared();
                *tail_guard = Some(segment.clone());
                segment
            };
            // The TS promise chain self-schedules; poll the segment on the
            // runtime so queued watcher invocations actually run.
            tokio::spawn(segment.clone());
            self.pending_tails.lock().push(segment);
        }
        // Fan the event out one listener at a time; INVARIANT failures
        // rethrow after every listener ran, others are contained.
        let args: Vec<ArcValue> = vec![
            arc(registration.ns.clone()),
            arc(next.clone()),
            arc(prev.clone()),
            arc(source.as_str()),
        ];
        let listeners = self
            .ctx
            .collect(DispatchMode::Emit, SETTINGS_UPDATED, &args);
        let mut invariant_failure: Option<String> = None;
        for (listener_ctx, listener) in listeners {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                    self.warn_listener_failure(&registration.ns, render_panic(payload));
                }
            }
        }
        if let Some(failure) = invariant_failure {
            panic!("{failure}");
        }
    }

    fn warn_listener_failure(&self, ns: &SettingsNamespace, error: String) {
        self.ctx
            .named_logger(Some("settings"))
            .warn(vec![arc(format!(
                "a settings/updated listener for \"{}\" failed: {error}",
                ns.as_str()
            ))]);
    }
}

/// One path-addressed edit to a namespace's user section (TS
/// `SettingsPathOp`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum SettingsPathOp {
    Set {
        path: Vec<String>,
        value: serde_json::Value,
    },
    Unset {
        path: Vec<String>,
    },
}

/// Apply one path op to a detached section (TS `applyPathOp`).
fn apply_path_op(section: Data, op: &Data) -> Result<Data, String> {
    let Data::Object(payload) = op else {
        return Err("settings mutate ops must be objects".to_string());
    };
    let kind = payload.get("op").and_then(|op| match op {
        Data::String(value) => Some(value.as_str()),
        _ => None,
    });
    let path = payload.get("path");
    let Data::Array(path) = path.unwrap_or(&Data::Undefined) else {
        return Err("settings mutate op paths must be arrays of strings".to_string());
    };
    let mut parts: Vec<String> = Vec::new();
    for part in path {
        match part {
            Data::String(value) => parts.push(value.clone()),
            _ => return Err("settings mutate op paths must be arrays of strings".to_string()),
        }
    }
    match kind {
        Some("set") => {
            let value = payload.get("value").cloned().unwrap_or(Data::Undefined);
            apply_set(section, &parts, &value)
        }
        Some("unset") => Ok(apply_unset(section, &parts)),
        _ => Err("settings mutate ops must be {op:'set'|'unset', path}".to_string()),
    }
}

fn apply_set(mut section: Data, path: &[String], value: &Data) -> Result<Data, String> {
    if path.is_empty() {
        // The empty path addresses the section itself.
        if !value.is_object() {
            return Err(
                "settings mutate: setting the section root requires a plain object".to_string(),
            );
        }
        return Ok(value.clone());
    }
    let head = &path[0];
    if path.len() == 1 {
        let Data::Object(object) = &mut section else {
            return Err("settings mutate: section must be an object".to_string());
        };
        object.insert(head.clone(), value.clone());
        return Ok(section);
    }
    let Data::Object(object) = &mut section else {
        return Err("settings mutate: section must be an object".to_string());
    };
    let child = object.get(head).cloned().unwrap_or(Data::Undefined);
    if !child.is_object() {
        // Setting through a non-object path creates the intermediate
        // objects it needs.
        let rebuilt = apply_set(Data::Object(IndexMap::new()), &path[1..], value)?;
        object.insert(head.clone(), rebuilt);
        return Ok(section);
    }
    let rebuilt = apply_set(child, &path[1..], value)?;
    object.insert(head.clone(), rebuilt);
    Ok(section)
}

fn apply_unset(mut section: Data, path: &[String]) -> Data {
    if path.is_empty() {
        return Data::Object(IndexMap::new());
    }
    let head = &path[0];
    if path.len() == 1 {
        if let Data::Object(object) = &mut section {
            object.shift_remove(head);
        }
        return section;
    }
    let Data::Object(object) = &mut section else {
        return section;
    };
    let Some(child) = object.get(head).cloned() else {
        return section;
    };
    if !child.is_object() {
        // Unsetting through an absent path is already satisfied.
        return section;
    }
    let rebuilt = apply_unset(child, &path[1..]);
    object.insert(head.clone(), rebuilt);
    section
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Merge,
    Replace,
    Mutate,
}

impl WriteMode {
    fn verb(&self) -> &'static str {
        match self {
            WriteMode::Merge => "update",
            WriteMode::Replace => "replace",
            WriteMode::Mutate => "mutate",
        }
    }
}

/// Convert a JSON value into a validation `Data` (the `serde_json` input
/// type already guarantees JSON compatibility).
fn data_from_json(value: &serde_json::Value) -> Result<Data, &'static str> {
    match value {
        serde_json::Value::Null => Ok(Data::Null),
        serde_json::Value::Bool(value) => Ok(Data::Bool(*value)),
        serde_json::Value::Number(value) => value
            .as_f64()
            .map(Data::Number)
            .ok_or("a non-finite number"),
        serde_json::Value::String(value) => Ok(Data::String(value.clone())),
        serde_json::Value::Array(array) => {
            let mut entries = Vec::with_capacity(array.len());
            for entry in array {
                entries.push(data_from_json(entry)?);
            }
            Ok(Data::Array(entries))
        }
        serde_json::Value::Object(object) => {
            let mut entries = IndexMap::new();
            for (key, entry) in object {
                entries.insert(key.clone(), data_from_json(entry)?);
            }
            Ok(Data::Object(entries))
        }
    }
}

/// Layer `over` onto `under`: plain objects merge recursively, every other
/// value (arrays included) replaces the lower layer wholesale.
fn merge_layers(under: &Data, over: &Data) -> Data {
    if over.is_nullish() {
        return under.clone();
    }
    if !under.is_object() || !over.is_object() {
        return over.clone();
    }
    let Data::Object(under_object) = under else {
        unreachable!()
    };
    let Data::Object(over_object) = over else {
        unreachable!()
    };
    let mut merged = under_object.clone();
    for (key, value) in over_object {
        match merged.get(key) {
            Some(existing) => {
                merged.insert(key.clone(), merge_layers(existing, value));
            }
            None => {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    Data::Object(merged)
}

/// Whether a panic payload is an invariant failure (code `INVARIANT`).
fn is_invariant_failure(payload: &(dyn std::any::Any + Send)) -> bool {
    if let Some(error) = payload.downcast_ref::<dsh_invariants::InvariantError>() {
        return error.code == "INVARIANT";
    }
    false
}

/// Render a panic payload to a string (downcast chain like dsh-session).
fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return message.to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "listener panicked".to_string()
}

/// Hooks a consumer hands to [`install_settings_section`].
pub struct SettingsSectionHooks {
    /// Receive the active configuration source thunk.
    pub set_source: Arc<dyn Fn(Arc<dyn Fn() -> Data + Send + Sync>) + Send + Sync>,
    /// Re-judge anything derived from the source.
    pub on_change: Arc<dyn Fn() + Send + Sync>,
    /// Reject a resolved section the consumer could not act on.
    pub validate: Option<Arc<dyn Fn(&Data) -> Result<(), String> + Send + Sync>>,
}

/// Install the canonical optional-settings consumer wiring (TS
/// `installSettingsSection`).
pub fn install_settings_section(
    ctx: &Context,
    ns: SettingsNamespace,
    schema: Schema,
    entry: Data,
    hooks: SettingsSectionHooks,
) -> Arc<cordis::FiberCore> {
    let ctx_for_unload = ctx.clone();
    let entry_for_fallback = entry.clone();
    let hooks_for_callback = Arc::new(hooks);
    ctx.inject(
        InjectSpec::new(["settings"]),
        Arc::new(move |sctx: &Context, _config: ArcValue| {
            let hooks = Arc::clone(&hooks_for_callback);
            let entry = entry_for_fallback.clone();
            let ns = ns.clone();
            let schema = schema.clone();
            let ctx_for_unload = ctx_for_unload.clone();
            let sctx = sctx.clone();
            Box::pin(async move {
                let provider: Arc<Arc<SettingsProvider>> = sctx
                    .get_typed::<Arc<SettingsProvider>>("settings", false)
                    .ok_or_else(|| {
                        cordis::PluginError::new(arc("settings service is not configured"))
                    })?;
                let scope = provider
                    .register(
                        &sctx,
                        ns.clone(),
                        schema,
                        SettingsRegisterOptions {
                            base: Some(entry.clone()),
                            validate: hooks.validate.clone(),
                            ..Default::default()
                        },
                    )
                    .map_err(|error| cordis::PluginError::new(arc(error)))?;
                (hooks.set_source)(Arc::new({
                    let scope = scope.clone();
                    move || (scope.get)()
                }));
                let hooks_for_dispose = Arc::clone(&hooks);
                let entry_for_dispose = entry.clone();
                let ctx_for_setup = ctx_for_unload.clone();
                let ctx_for_watch = ctx_for_unload.clone();
                let _ = sctx.effect(
                    &format!("settings section {ns} fallback"),
                    Box::pin(async move {
                        Some(make_disposer(move || {
                            let hooks = Arc::clone(&hooks_for_dispose);
                            let entry = entry_for_dispose.clone();
                            let ctx_for_unload = ctx_for_setup.clone();
                            Box::pin(async move {
                                // The consumer's own unload needs no fallback;
                                // only a settings-provider detach does.
                                if is_unloading(&ctx_for_unload) {
                                    return;
                                }
                                (hooks.set_source)(Arc::new(move || entry.clone()));
                                (hooks.on_change)();
                            })
                        }))
                    }),
                );
                (hooks.on_change)();
                let watch_hooks = Arc::clone(&hooks);
                let watch_scope = scope.clone();
                (watch_scope.watch)(Arc::new(move |_next, _prev| {
                    let hooks = Arc::clone(&watch_hooks);
                    let ctx_for_unload = ctx_for_watch.clone();
                    Box::pin(async move {
                        if is_unloading(&ctx_for_unload) {
                            return;
                        }
                        (hooks.on_change)();
                    })
                }));
                Ok(())
            })
        }),
    )
}

/// Whether the consumer's own fiber is tearing down (TS `isUnloading`).
fn is_unloading(ctx: &Context) -> bool {
    matches!(
        ctx.fiber.state(),
        cordis::FiberState::Unloading | cordis::FiberState::Disposed
    )
}
