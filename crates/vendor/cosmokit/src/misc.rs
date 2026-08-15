//! Object/entry helpers (port of `src/misc.ts`).

use indexmap::IndexMap;
use serde_json::Value;

/// No-op callback (TS `noop()` returns `undefined`; Rust returns `()`).
pub fn noop() {}

/// Return true for JSON `null` (Rust replacement for `isNullable` at the
/// JSON-value layer; `Option` covers the Rust-native case).
pub fn is_null(value: &Value) -> bool {
    value.is_null()
}

/// Return true for non-array object values (JSON objects).
pub fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

/// Filter object entries and return a new object.
pub fn filter_keys<K, V, F>(object: &IndexMap<K, V>, mut filter: F) -> IndexMap<K, V>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
    F: FnMut(&K, &V) -> bool,
{
    object
        .iter()
        .filter(|(key, value)| filter(key, value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Map object values while preserving the original key set.
pub fn map_values<K, V, U, F>(object: &IndexMap<K, V>, mut transform: F) -> IndexMap<K, U>
where
    K: Eq + std::hash::Hash + Clone,
    F: FnMut(&V, &K) -> U,
{
    object
        .iter()
        .map(|(key, value)| (key.clone(), transform(value, key)))
        .collect()
}

/// Alias for [`map_values`] (TS `valueMap`).
pub use map_values as value_map;

/// Pick selected keys from an object, optionally including `None` values
/// (TS `pick(source, keys, forced)`).
///
/// `None` values are skipped unless `forced` is set.
pub fn pick<K, V, I>(source: &IndexMap<K, Option<V>>, keys: Option<I>, forced: bool) -> IndexMap<K, Option<V>>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
    I: IntoIterator<Item = K>,
{
    match keys {
        None => source.clone(),
        Some(keys) => {
            let mut result = IndexMap::new();
            for key in keys {
                if let Some(value) = source.get(&key)
                    && (forced || value.is_some())
                {
                    result.insert(key, value.clone());
                }
            }
            result
        }
    }
}

/// Omit selected keys from a shallow object copy.
pub fn omit<K, V, I>(source: &IndexMap<K, V>, keys: Option<I>) -> IndexMap<K, V>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
    I: IntoIterator<Item = K>,
{
    match keys {
        None => source.clone(),
        Some(keys) => {
            let mut result = source.clone();
            for key in keys {
                result.shift_remove(&key);
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, i32)]) -> IndexMap<String, i32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn filters_and_maps() {
        let source = map(&[("a", 1), ("b", 2), ("c", 3)]);
        let filtered = filter_keys(&source, |key, _| key != "b");
        assert_eq!(filtered, map(&[("a", 1), ("c", 3)]));
        let mapped = map_values(&source, |value, key| format!("{key}:{value}"));
        assert_eq!(
            mapped,
            [("a".to_string(), "a:1".to_string()), ("b".into(), "b:2".into()), ("c".into(), "c:3".into())]
                .into_iter()
                .collect::<IndexMap<_, _>>()
        );
        assert_eq!(value_map(&source, |v, _| *v), source);
    }

    #[test]
    fn pick_and_omit() {
        let source: IndexMap<String, Option<i32>> = [("a", Some(1)), ("b", None), ("c", Some(3))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let picked = pick(&source, Some(["a".to_string(), "b".to_string(), "x".to_string()]), false);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked.get("a"), Some(&Some(1)));
        let forced = pick(&source, Some(["b".to_string()]), true);
        assert_eq!(forced.get("b"), Some(&None));

        let plain = map(&[("a", 1), ("b", 2)]);
        assert_eq!(omit(&plain, Some(["b".to_string()])), map(&[("a", 1)]));
        assert_eq!(omit(&plain, None::<std::vec::IntoIter<String>>), plain);
    }
}
