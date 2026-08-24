//! Shared internal helpers used by context, services, and plugin fibers.
//!
//! Rust port of `vendor/cordis/src/utils.ts`.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Boxed `Send` future alias used across the crate.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type-erased service/argument value. Mirrors cordis' dynamic `any` values.
pub type ArcValue = Arc<dyn Any + Send + Sync>;

/// Construct an [`ArcValue`] from any `Send + Sync + 'static` value.
pub fn arc<T: Any + Send + Sync>(value: T) -> ArcValue {
    Arc::new(value)
}

/// Downcast an [`ArcValue`] to a concrete reference.
pub fn downcast<T: Any + Send + Sync>(value: &ArcValue) -> Option<&T> {
    value.downcast_ref::<T>()
}

/// Downcast an [`ArcValue`] to a cloned `Arc<T>`.
pub fn downcast_arc<T: Any + Send + Sync>(value: &ArcValue) -> Option<Arc<T>> {
    value.clone().downcast::<T>().ok()
}

/// Shared symbol names used by the TS runtime (`cordis.*`). Kept as a marker
/// for diagnostics and for future cross-realm interop.
pub mod symbols {
    pub const SHADOW: &str = "cordis.shadow";
    pub const RECEIVER: &str = "cordis.receiver";
    pub const ORIGINAL: &str = "cordis.original";
    pub const EFFECT: &str = "cordis.effect";
    pub const FILTER: &str = "cordis.filter";
    pub const ISOLATE: &str = "cordis.isolate";
    pub const INTERCEPT: &str = "cordis.intercept";
    pub const INIT: &str = "cordis.init";
    pub const CHECK: &str = "cordis.check";
    pub const CONFIG: &str = "cordis.config";
    pub const INVOKE: &str = "cordis.invoke";
    pub const EXTEND: &str = "cordis.extend";
    pub const TRACKER: &str = "cordis.tracker";
    pub const RESOLVE_CONFIG: &str = "cordis.resolveConfig";
}

/// Return whether an event result should stop a bail-style dispatch.
///
/// Port of `isBailed`: `true` unless the value is absent.
pub fn is_bailed(value: Option<&ArcValue>) -> bool {
    value.is_some()
}

/// Ordered collection of disposable values with O(1) deletion by value.
///
/// Port of `DisposableList` (`utils.ts`). Entries are `Arc` handles so removal
/// can use pointer identity, matching the TS `WeakMap` keyed by value.
pub struct DisposableList<T: ?Sized> {
    sn: AtomicU64,
    entries: parking_lot::Mutex<Vec<(u64, Arc<T>)>>,
}

impl<T: ?Sized> Default for DisposableList<T> {
    fn default() -> Self {
        Self {
            sn: AtomicU64::new(0),
            entries: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl<T: ?Sized> DisposableList<T> {
    /// Number of stored values.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append a value; returns a closure removing exactly this entry.
    pub fn push(&self, value: Arc<T>) -> impl Fn() + '_ {
        let sn = self.sn.fetch_add(1, Ordering::Relaxed) + 1;
        self.entries.lock().push((sn, value));
        move || {
            let mut entries = self.entries.lock();
            if let Some(pos) = entries.iter().position(|(n, _)| *n == sn) {
                entries.remove(pos);
            }
        }
    }

    /// Remove the entry holding exactly this `Arc` (pointer identity).
    pub fn delete(&self, value: &Arc<T>) -> bool {
        let mut entries = self.entries.lock();
        let before = entries.len();
        entries.retain(|(_, v)| !Arc::ptr_eq(v, value));
        entries.len() != before
    }

    /// Remove every entry and return them in reverse registration order.
    pub fn clear(&self) -> Vec<Arc<T>> {
        let mut entries = self.entries.lock();
        let values: Vec<Arc<T>> = entries.iter().map(|(_, v)| v.clone()).rev().collect();
        entries.clear();
        values
    }

    /// Snapshot of entries in registration order.
    pub fn snapshot(&self) -> Vec<Arc<T>> {
        self.entries.lock().iter().map(|(_, v)| v.clone()).collect()
    }
}

/// Extract a non-empty text message out of an error value (for logging).
pub fn error_message(reason: &ArcValue) -> String {
    if let Some(err) = reason.downcast_ref::<anyhow::Error>() {
        return format!("{err:#}");
    }
    if let Some(msg) = reason.downcast_ref::<String>() {
        return msg.clone();
    }
    if let Some(msg) = reason.downcast_ref::<&'static str>() {
        return (*msg).to_string();
    }
    "unknown error".to_string()
}

/// Lazy epoch marker used by fibers: `None` represents the INACTIVE epoch.
pub type Epoch = Option<String>;

/// Compose an epoch string from dependency implementation uids.
pub fn compose_epoch(uids: impl IntoIterator<Item = u64>) -> Epoch {
    let parts: Vec<String> = uids.into_iter().map(|u| u.to_string()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(":"))
    }
}

/// Persistent overlay map used for isolation labels and intercept configs:
/// child entries shadow the parent without mutating it (TS prototype chain).
pub struct OverlayMap<V: Clone> {
    pub parent: Option<Arc<OverlayMap<V>>>,
    entries: parking_lot::Mutex<HashMap<String, V>>,
}

impl<V: Clone> Default for OverlayMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone> OverlayMap<V> {
    pub fn new() -> Self {
        Self {
            parent: None,
            entries: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    pub fn child(parent: &Arc<OverlayMap<V>>) -> Self {
        Self {
            parent: Some(parent.clone()),
            entries: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, key: String, value: V) {
        self.entries.lock().insert(key, value);
    }

    /// Insert `value` only if `key` is absent from this map's own entries;
    /// returns the winning value (existing or new).
    pub fn insert_if_absent(&self, key: &str, value: V) -> V {
        let mut entries = self.entries.lock();
        if let Some(existing) = entries.get(key) {
            return existing.clone();
        }
        entries.insert(key.to_string(), value.clone());
        value
    }

    /// Look up a key, walking the ancestor chain.
    pub fn get(&self, key: &str) -> Option<V> {
        let mut current = Some(self);
        while let Some(map) = current {
            if let Some(value) = map.entries.lock().get(key) {
                return Some(value.clone());
            }
            current = map.parent.as_deref();
        }
        None
    }

    /// Collect the chain of values for `key`, ancestors first (root → leaf).
    pub fn chain(&self, key: &str) -> Vec<V> {
        let mut stack: Vec<&OverlayMap<V>> = Vec::new();
        let mut current = Some(self);
        while let Some(map) = current {
            if map.entries.lock().contains_key(key) {
                stack.push(map);
            }
            current = map.parent.as_deref();
        }
        stack.reverse();
        stack
            .into_iter()
            .map(|map| map.entries.lock().get(key).cloned().unwrap())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposable_list_reverses_on_clear() {
        let list = DisposableList::<str>::default();
        let a: Arc<str> = Arc::from("a");
        let b: Arc<str> = Arc::from("b");
        let _ = list.push(a.clone());
        let _ = list.push(b.clone());
        assert_eq!(list.len(), 2);
        let cleared = list.clear();
        assert_eq!(cleared.len(), 2);
        assert_eq!(*cleared[0], *b);
        assert_eq!(*cleared[1], *a);
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn disposable_list_delete_by_identity() {
        let list = DisposableList::<str>::default();
        let a: Arc<str> = Arc::from("a");
        let _ = list.push(a.clone());
        list.delete(&a);
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn overlay_map_child_shadowing() {
        let root = Arc::new(OverlayMap::<String>::new());
        root.insert("x".into(), "root".into());
        let child = Arc::new(OverlayMap::child(&root));
        child.insert("x".into(), "child".into());
        assert_eq!(child.get("x"), Some("child".to_string()));
        let grandchild = Arc::new(OverlayMap::child(&child));
        assert_eq!(grandchild.get("x"), Some("child".to_string()));
        assert_eq!(
            grandchild.chain("x"),
            vec!["root".to_string(), "child".to_string()]
        );
    }
}
