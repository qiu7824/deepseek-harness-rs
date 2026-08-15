//! Shared insertion-ordered storage and effect ownership for scope-aware
//! registries (port of `src/store.ts`).
//!
//! Tables live behind `Arc` so undo closures can be `'static` (required by
//! cordis effect disposers) while remaining tied to the exact registration.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cordis::{Context, Disposer, make_disposer};
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::{ScopeKey, scope_chain_of, scope_of};

/// One scope's aggregate contribution to a registry.
pub trait ScopeLayer {
    /// Whether every table in this layer is empty.
    fn is_empty(&self) -> bool;
}

/// Insertion-ordered named entries with caller-owned duplicate diagnostics
/// (TS `NamedEntries`).
#[derive(Clone)]
pub struct NamedEntries<V: Clone> {
    data: Arc<Mutex<IndexMap<String, V>>>,
    duplicate_error: Arc<dyn Fn(&str) -> Box<dyn std::error::Error + Send + Sync> + Send + Sync>,
}

impl<V: Clone + Send + Sync + 'static> NamedEntries<V> {
    pub fn new(
        duplicate_error: impl Fn(&str) -> Box<dyn std::error::Error + Send + Sync> + Send + Sync + 'static,
    ) -> Self {
        Self {
            data: Arc::new(Mutex::new(IndexMap::new())),
            duplicate_error: Arc::new(duplicate_error),
        }
    }

    /// Insert one unique name; returns an idempotent undo removing only this
    /// insertion (TS `insert`).
    pub fn insert(&self, name: &str, value: V) -> Box<dyn Fn() + Send + Sync> {
        {
            let mut data = self.data.lock();
            if data.contains_key(name) {
                panic!("{}", (self.duplicate_error)(name));
            }
            data.insert(name.to_string(), value);
        }
        let data = self.data.clone();
        let name = name.to_string();
        let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
        Box::new(move || {
            if !active.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            data.lock().shift_remove(&name);
        })
    }

    pub fn get(&self, name: &str) -> Option<V> {
        self.data.lock().get(name).cloned()
    }

    pub fn has(&self, name: &str) -> bool {
        self.data.lock().contains_key(name)
    }

    pub fn keys(&self) -> Vec<String> {
        self.data.lock().keys().cloned().collect()
    }

    pub fn entries(&self) -> Vec<(String, V)> {
        self.data
            .lock()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Number of currently stored entries.
    pub fn len(&self) -> usize {
        self.data.lock().len()
    }

    /// Read the entry at an insertion-order index without holding the lock
    /// across the caller's use of the value. Combined with [`Self::len`],
    /// this supports LIVE iteration: entries inserted while the caller
    /// processes an earlier entry (appended at the end) are picked up in
    /// the same pass, matching the TS live `Map.entries()` iterator.
    pub fn get_index(&self, index: usize) -> Option<(String, V)> {
        self.data
            .lock()
            .get_index(index)
            .map(|(key, value)| (key.clone(), value.clone()))
    }

    pub fn values(&self) -> Vec<V> {
        self.data.lock().values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.data.lock().is_empty()
    }
}

impl<V: Clone + Send + Sync + 'static> ScopeLayer for NamedEntries<V> {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }
}

/// Insertion-ordered anonymous entries with independent registration identity
/// (TS `AnonymousEntries`).
pub struct AnonymousEntries<V: Clone> {
    data: Arc<Mutex<IndexMap<u64, V>>>,
    counter: AtomicU64,
}

impl<V: Clone> Clone for AnonymousEntries<V> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            counter: AtomicU64::new(0),
        }
    }
}

impl<V: Clone + Send + Sync + 'static> Default for AnonymousEntries<V> {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(IndexMap::new())),
            counter: AtomicU64::new(0),
        }
    }
}

impl<V: Clone + Send + Sync + 'static> AnonymousEntries<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one independently owned value; returns an idempotent undo
    /// (TS `append`).
    pub fn append(&self, value: V) -> Box<dyn Fn() + Send + Sync> {
        let key = self.counter.fetch_add(1, Ordering::Relaxed);
        self.data.lock().insert(key, value);
        let data = self.data.clone();
        let active = Arc::new(AtomicBool::new(true));
        Box::new(move || {
            if !active.swap(false, Ordering::SeqCst) {
                return;
            }
            data.lock().shift_remove(&key);
        })
    }

    pub fn values(&self) -> Vec<V> {
        self.data.lock().values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.data.lock().is_empty()
    }
}

impl<V: Clone + Send + Sync + 'static> ScopeLayer for AnonymousEntries<V> {
    fn is_empty(&self) -> bool {
        self.is_empty()
    }
}

/// Own the global and exact-scope layers for one registry (TS
/// `ScopedLayers`).
pub struct ScopedLayers<L: ScopeLayer> {
    /// The eagerly constructed context-global layer.
    pub global: Arc<L>,
    scoped: Arc<Mutex<HashMap<usize, Arc<L>>>>,
    create_layer: Arc<dyn Fn(Option<&ScopeKey>) -> L + Send + Sync>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl<L: ScopeLayer + Send + Sync + 'static> ScopedLayers<L> {
    pub fn new(
        create_layer: impl Fn(Option<&ScopeKey>) -> L + Send + Sync + 'static,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            global: Arc::new(create_layer(None)),
            scoped: Arc::new(Mutex::new(HashMap::new())),
            create_layer: Arc::new(create_layer),
            on_change: Arc::new(on_change),
        }
    }

    /// Read an existing exact-scope overlay without creating one
    /// (TS `peek`).
    pub fn peek(&self, scope: Option<&ScopeKey>) -> Option<Arc<L>> {
        let scope = scope?;
        self.scoped.lock().get(&(scope.key_id() as usize)).cloned()
    }

    /// Existing overlays along the scope's parent chain, farthest ancestor
    /// first (TS `chainLayers`).
    pub fn chain_layers(&self, scope: Option<&ScopeKey>) -> Vec<Arc<L>> {
        let mut layers = Vec::new();
        for key in scope_chain_of(scope).into_iter().rev() {
            if let Some(layer) = self.scoped.lock().get(&(key.key_id() as usize)).cloned() {
                layers.push(layer);
            }
        }
        layers
    }

    /// Materialize global named entries followed by scope-chain shadows,
    /// nearest scope wins (TS `merge`).
    pub fn merge<V: Clone + Send + Sync + 'static>(
        &self,
        scope: Option<&ScopeKey>,
        pick: impl Fn(&L) -> &NamedEntries<V>,
    ) -> IndexMap<String, V> {
        let mut merged: IndexMap<String, V> =
            pick(&self.global).entries().into_iter().collect();
        for layer in self.chain_layers(scope) {
            for (name, value) in pick(&layer).entries() {
                merged.insert(name, value);
            }
        }
        merged
    }

    /// Attach one layer mutation to its registration context (TS `effect`).
    ///
    /// Reads never create scoped layers. The mutation runs synchronously in
    /// the caller (mirroring the TS generator body); its undo reclaims the
    /// entry and drops the layer once completely empty. `notify` fires after
    /// setup and after undo.
    pub fn effect(
        &self,
        ctx: &Context,
        action: impl Fn(&L) -> Box<dyn Fn() + Send + Sync> + Send + Sync + 'static,
        label: &str,
        notify: bool,
    ) -> Disposer {
        let scope_key = scope_of(ctx);
        let scope_id = scope_key.as_ref().map(|key| key.key_id() as usize);
        let scoped = self.scoped.clone();
        let create_layer = self.create_layer.clone();
        let on_change = self.on_change.clone();
        let global = self.global.clone();

        // Synchronous setup: resolve/create the layer and run the mutation
        // (TS runs the generator body synchronously inside `ctx.effect`).
        let (layer, created) = match scope_id {
            None => (global.clone(), false),
            Some(id) => {
                let mut layers = scoped.lock();
                match layers.get(&id).cloned() {
                    Some(layer) => (layer, false),
                    None => {
                        let layer = Arc::new(create_layer(scope_key.as_ref()));
                        layers.insert(id, layer.clone());
                        (layer, true)
                    }
                }
            }
        };
        let undo = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            action(&layer)
        })) {
            Ok(undo) => Arc::new(undo),
            Err(payload) => {
                if created {
                    if let Some(id) = scope_id {
                        let mut layers = scoped.lock();
                        let reclaim =
                            layers.get(&id).is_some_and(|layer| layer.is_empty());
                        if reclaim {
                            layers.remove(&id);
                        }
                    }
                }
                std::panic::resume_unwind(payload);
            }
        };
        if notify {
            // TS: a synchronous `system-prompt/change`-style listener throw
            // rolls the registration back (the generator effect disposes the
            // already-yielded undo on the throw path) and propagates.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_change())) {
                Ok(()) => {}
                Err(payload) => {
                    undo();
                    if created {
                        if let Some(id) = scope_id {
                            let mut layers = scoped.lock();
                            let reclaim = layers.get(&id).is_some_and(|layer| layer.is_empty());
                            if reclaim {
                                layers.remove(&id);
                            }
                        }
                    }
                    std::panic::resume_unwind(payload);
                }
            }
        }

        let on_change_for_dispose = on_change.clone();
        let scoped_for_dispose = scoped.clone();
        let disposer = make_disposer(move || {
            let undo = undo.clone();
            let on_change = on_change_for_dispose.clone();
            let scoped = scoped_for_dispose.clone();
            Box::pin(async move {
                undo();
                if let Some(id) = scope_id {
                    let mut layers = scoped.lock();
                    let reclaim = layers.get(&id).is_some_and(|layer| layer.is_empty());
                    if reclaim {
                        layers.remove(&id);
                    }
                }
                if notify {
                    on_change();
                }
            })
        });
        ctx.effect(label, Box::pin(async move { Some(disposer) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateScopeOptions, ScopeKey, create_scope};
    use std::sync::atomic::AtomicU32;

    fn duplicate_error(name: &str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("duplicate {name}"),
        ))
    }

    #[derive(Clone)]
    struct Layer {
        entries: NamedEntries<i32>,
    }

    impl Layer {
        fn new() -> Self {
            Self {
                entries: NamedEntries::new(duplicate_error),
            }
        }
    }

    impl ScopeLayer for Layer {
        fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }

    #[test]
    fn named_entries_operations() {
        let entries = NamedEntries::<i32>::new(duplicate_error);
        let undo_a = entries.insert("a", 1);
        let _ = entries.insert("b", 2);
        assert!(entries.has("a"));
        assert_eq!(entries.get("a"), Some(1));
        assert_eq!(entries.keys(), vec!["a".to_string(), "b".to_string()]);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            entries.insert("a", 9)
        }))
        .is_err());
        undo_a();
        undo_a(); // idempotent
        assert!(!entries.has("a"));
        assert_eq!(entries.values(), vec![2]);
    }

    #[test]
    fn anonymous_entries_keep_duplicates_separate() {
        let entries = AnonymousEntries::<i32>::new();
        let undo1 = entries.append(7);
        let _ = entries.append(7);
        assert_eq!(entries.values(), vec![7, 7]);
        undo1();
        assert_eq!(entries.values(), vec![7]);
        assert!(!entries.is_empty());
    }

    #[tokio::test]
    async fn scoped_layers_shadow_and_reclaim() {
        let changes = Arc::new(AtomicU32::new(0));
        let changes2 = changes.clone();
        let layers = Arc::new(ScopedLayers::<Layer>::new(
            |_scope| Layer::new(),
            move || {
                changes2.fetch_add(1, Ordering::SeqCst);
            },
        ));

        let ctx = Context::root();
        let key = ScopeKey::new();
        let scope = create_scope(&ctx, key.clone(), &CreateScopeOptions::default());
        assert!(layers.peek(Some(&key)).is_none());

        let dispose = layers.effect(&scope.ctx, |_layer| Box::new(|| {}), "scoped", true);
        assert!(layers.peek(Some(&key)).is_some());
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        let scoped_layer = layers.peek(Some(&key)).expect("scoped layer");
        let _ = scoped_layer.entries.insert("x", 1);
        let _ = layers.global.entries.insert("x", 0);
        let _ = layers.global.entries.insert("y", 2);
        let merged = layers.merge(Some(&key), |layer| &layer.entries);
        assert_eq!(merged.get("x"), Some(&1), "scoped shadows global");
        assert_eq!(merged.get("y"), Some(&2), "global visible through scope");

        // disposer keeps the layer while it still holds entries
        dispose().await;
        assert!(layers.peek(Some(&key)).is_some(), "non-empty layer survives dispose");
        assert_eq!(changes.load(Ordering::SeqCst), 2);

        // a second, empty scoped layer is reclaimed on dispose
        let empty_key = ScopeKey::new();
        let empty_scope = create_scope(&ctx, empty_key.clone(), &CreateScopeOptions::default());
        let empty_dispose = layers.effect(&empty_scope.ctx, |_layer| Box::new(|| {}), "empty", true);
        assert!(layers.peek(Some(&empty_key)).is_some());
        empty_dispose().await;
        assert!(layers.peek(Some(&empty_key)).is_none(), "empty layer reclaimed");
    }
}
