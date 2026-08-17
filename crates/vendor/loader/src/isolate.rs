//! Service isolation realms (port of `src/config/isolate.ts`).
//!
//! TS uses per-entry `Symbol` labels to isolate service implementations;
//! Rust cordis isolation labels are `u64`s (see `Context::isolate`), so a
//! realm maps service names to label ids. `LocalRealm` is entry-scoped,
//! `GlobalRealm` is shared by entries using the same label string.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// Realm labels come from cordis's shared isolation-label namespace so they
/// never collide with labels allocated by the context machinery itself.
fn next_label() -> u64 {
    cordis::allocate_isolation_label()
}

/// Label store for one realm (TS `Realm`).
pub struct Realm {
    store: Mutex<HashMap<String, u64>>,
}

impl Realm {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve the label for `key`. With `create`, the label is persisted in
    /// the realm; otherwise an ephemeral label is returned (TS `access`).
    pub fn access(&self, key: &str, create: bool) -> u64 {
        let mut store = self.store.lock();
        if let Some(label) = store.get(key) {
            return *label;
        }
        let label = next_label();
        if create {
            store.insert(key.to_string(), label);
        }
        label
    }

    pub fn delete(&self, key: &str) {
        self.store.lock().remove(key);
    }

    pub fn size(&self) -> usize {
        self.store.lock().len()
    }
}

/// Entry-local isolation realm (TS `LocalRealm`, suffix `#<entry id>`).
pub struct LocalRealm {
    pub entry_id: String,
    pub realm: Realm,
}

impl LocalRealm {
    pub fn new(entry_id: String) -> Arc<Self> {
        Arc::new(Self {
            entry_id,
            realm: Realm::new(),
        })
    }
}

/// Named isolation realm shared by entries with the same label
/// (TS `GlobalRealm`, suffix `@<label>`).
pub struct GlobalRealm {
    pub label: String,
    pub realm: Realm,
}

impl GlobalRealm {
    pub fn new(label: String) -> Arc<Self> {
        Arc::new(Self {
            label,
            realm: Realm::new(),
        })
    }
}
