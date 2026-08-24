//! Scoped-context primitives: Rust port of `@deepseek-ai/dsh-scope`.
//!
//! Mints a Cordis context that tags registrations with an opaque identity
//! and builds routing-only event carriers for that identity. Registration
//! views inherit DOWN the scope chain ([`ScopedLayers`]); event admission
//! extends UP it ([`scope_target`]).
//!
//! # Deviations
//!
//! - `ScopeKey` is an `Arc<unit>` identity (`Object` in TS); the parent
//!   registry is a process-global table keyed by Arc pointers and never
//!   garbage-collects keys (TS uses `WeakMap`).
//! - The scope tag travels through a fiber-keyed table instead of a context
//!   symbol property (child contexts share the fiber, so lookups agree).
//! - [`scope_target`] returns a typed carrier struct; event dispatch uses
//!   `ctx.with_filter(carrier.filter)`.

pub mod store;

pub use store::{AnonymousEntries, NamedEntries, PreparedRegistration, ScopeLayer, ScopedLayers};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::{BoxFuture, Context, Plugin, PluginError, arc};
use parking_lot::Mutex;

static NEXT_KEY_ID: AtomicU64 = AtomicU64::new(1);

/// An opaque, identity-compared scope key (TS `ScopeKey = object`).
///
/// The identity is a process-unique monotonic id, NOT the allocation
/// address: the parent registry keeps entries for the process lifetime
/// (TS uses a `WeakMap`; this port documents strong retention), so a
/// dropped key's recycled address must never alias a live one.
#[derive(Debug, Clone)]
pub struct ScopeKey {
    id: u64,
    _anchor: Arc<()>,
}

impl ScopeKey {
    pub fn new() -> Self {
        Self {
            id: NEXT_KEY_ID.fetch_add(1, Ordering::Relaxed),
            _anchor: Arc::new(()),
        }
    }

    pub(crate) fn key_id(&self) -> u64 {
        self.id
    }
}

impl PartialEq for ScopeKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ScopeKey {}

impl std::hash::Hash for ScopeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Default for ScopeKey {
    fn default() -> Self {
        Self::new()
    }
}

/// The enclosing-scope registry (TS `scopeParents` WeakMap; Rust keeps strong
/// refs — keys live for the process lifetime, matching the WeakMap in
/// practice since every chain consumer holds keys).
static SCOPE_PARENTS: LazyLock<Mutex<HashMap<u64, ScopeKey>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Scope tags by fiber pointer (TS `[kScope]` context property). Entries are
/// removed when the scope fiber disposes (see [`forget_scope_fiber`]), so a
/// recycled fiber address can never observe a stale tag.
static SCOPE_TAGS: LazyLock<Mutex<HashMap<usize, ScopeKey>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn link_scope_parent(key: &ScopeKey, parent: &ScopeKey) {
    let mut parents = SCOPE_PARENTS.lock();
    let mut cursor = Some(parent.clone());
    while let Some(current) = cursor {
        if current.key_id() == key.key_id() {
            panic!("dsh-scope: scope parent link would form a cycle");
        }
        cursor = parents.get(&current.key_id()).cloned();
    }
    parents.insert(key.key_id(), parent.clone());
}

/// The privileged handle to move one scope key's parent link.
#[derive(Clone)]
pub struct ScopeParentBinding {
    key: ScopeKey,
}

impl ScopeParentBinding {
    /// Re-link the bound key to a different parent, with the same cycle check
    /// as the bind (TS `rebind`).
    pub fn rebind(&self, parent: &ScopeKey) {
        link_scope_parent(&self.key, parent);
    }
}

/// Bind `parent` as `key`'s enclosing scope, once (TS `bindScopeParent`).
pub fn bind_scope_parent(key: &ScopeKey, parent: &ScopeKey) -> ScopeParentBinding {
    {
        let parents = SCOPE_PARENTS.lock();
        if parents.contains_key(&key.key_id()) {
            panic!(
                "dsh-scope: scope key is already bound to a parent; re-linking requires the binding returned by the original bind"
            );
        }
    }
    link_scope_parent(key, parent);
    ScopeParentBinding { key: key.clone() }
}

/// Read one key's enclosing scope (TS `scopeParentOf`).
pub fn scope_parent_of(key: &ScopeKey) -> Option<ScopeKey> {
    SCOPE_PARENTS.lock().get(&key.key_id()).cloned()
}

/// The chain from a key to its root ancestor, nearest first
/// (TS `scopeChainOf`).
pub fn scope_chain_of(key: Option<&ScopeKey>) -> Vec<ScopeKey> {
    let mut chain = Vec::new();
    let parents = SCOPE_PARENTS.lock();
    let mut cursor = key.cloned();
    while let Some(current) = cursor {
        chain.push(current.clone());
        cursor = parents.get(&current.key_id()).cloned();
    }
    chain
}

/// A minted registration scope and its disposal boundaries (TS `Scope`).
pub struct Scope {
    /// Context through which scope-owned registrations are made.
    pub ctx: Context,
    /// Exact Cordis disposer (TS `rawDispose`).
    pub raw_dispose: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
    /// Dispose once; racing calls await the same completion (TS `dispose`).
    pub dispose: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>,
}

type ContextFilter = dyn Fn(&Context) -> bool + Send + Sync;

/// Options accepted by [`create_scope`].
#[derive(Default)]
pub struct CreateScopeOptions {
    /// Enclosing scope bound via [`bind_scope_parent`] before the scope is
    /// usable.
    pub parent: Option<ScopeKey>,
}

struct ScopePlugin;

#[async_trait::async_trait]
impl Plugin for ScopePlugin {
    async fn apply(&self, _ctx: &Context, _config: cordis::ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Mint a scope under `ctx` (TS `createScope`).
pub fn create_scope(ctx: &Context, key: ScopeKey, options: &CreateScopeOptions) -> Scope {
    if let Some(parent) = &options.parent {
        bind_scope_parent(&key, parent);
    }
    let fiber = ctx.plugin(Arc::new(ScopePlugin), arc(()));
    {
        let mut tags = SCOPE_TAGS.lock();
        tags.insert(Arc::as_ptr(&fiber) as *const () as usize, key);
    }
    let fiber_ctx = fiber.ctx().expect("scope fiber context").extend();
    let raw_fiber = fiber.clone();
    let raw_dispose: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync> = Arc::new(move || {
        let fiber = raw_fiber.clone();
        Box::pin(async move {
            forget_scope_fiber(&fiber);
            fiber.dispose().await;
        })
    });
    let dispose_cell = Arc::new(tokio::sync::OnceCell::new());
    let raw_for_dispose = raw_dispose.clone();
    let dispose: Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync> = Arc::new(move || {
        let cell = dispose_cell.clone();
        let raw = raw_for_dispose.clone();
        Box::pin(async move {
            let _ = cell.get_or_init(|| async { raw().await }).await;
        })
    });
    Scope {
        ctx: fiber_ctx,
        raw_dispose,
        dispose,
    }
}

/// Read the nearest scope tag inherited by a context (TS `scopeOf`). The
/// lookup walks the fiber parent chain: a plugin mounted under a scope
/// context receives a child fiber whose own tag is absent, and the TS
/// composition still resolves the enclosing scope. The root fiber binds
/// itself as its own parent, so the walk is cycle-guarded.
pub fn scope_of(ctx: &Context) -> Option<ScopeKey> {
    let tags = SCOPE_TAGS.lock();
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut fiber = Some(ctx.fiber.clone());
    while let Some(current) = fiber {
        if !seen.insert(Arc::as_ptr(&current) as *const () as usize) {
            break;
        }
        if let Some(key) = tags.get(&(Arc::as_ptr(&current) as *const () as usize)) {
            return Some(key.clone());
        }
        fiber = current.parent_ctx().map(|parent| parent.fiber.clone());
    }
    None
}

/// Remove the scope tag when a scope fiber is disposed (host bookkeeping;
/// fibers whose scopes outlive the process are never reclaimed).
pub fn forget_scope_fiber(fiber: &Arc<cordis::FiberCore>) {
    SCOPE_TAGS
        .lock()
        .remove(&(Arc::as_ptr(fiber) as *const () as usize));
}

/// A routing-only event carrier (TS `scopeTarget` + `Scoped<T>`).
#[derive(Clone)]
pub struct ScopeCarrier {
    /// The dispatch filter preserving the base filter and admitting
    /// listeners tagged with the key or any of its ancestors.
    pub filter: Arc<dyn Fn(&Context) -> bool + Send + Sync>,
    /// The routed scope identity, or `None` for an unscoped subject.
    pub key: Option<ScopeKey>,
}

/// Build an opaque receiver (TS `scopeTarget`).
pub fn scope_target(base: Option<Arc<ContextFilter>>, key: Option<ScopeKey>) -> ScopeCarrier {
    let key_for_filter = key.clone();
    let filter: Arc<dyn Fn(&Context) -> bool + Send + Sync> = Arc::new(move |ctx| {
        if let Some(base) = &base
            && !base(ctx)
        {
            return false;
        }
        let Some(tag) = scope_of(ctx) else {
            return true;
        };
        let Some(key) = &key_for_filter else {
            return false;
        };
        for cursor in scope_chain_of(Some(key)) {
            if cursor.key_id() == tag.key_id() {
                return true;
            }
        }
        false
    });
    ScopeCarrier { filter, key }
}

/// Test whether a value is a scope carrier (TS `isScopeCarrier`; Rust
/// carriers are always [`ScopeCarrier`] values).
pub fn carrier_key_of(carrier: &ScopeCarrier) -> Option<&ScopeKey> {
    carrier.key.as_ref()
}
