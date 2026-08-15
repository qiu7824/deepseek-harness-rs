//! Lossless-JSON validation and detached snapshots for durable session data.
//! Rust port of `packages/core/session/src/json.ts`.
//!
//! Rust's `serde_json::Value` cannot represent `undefined`, `NaN`,
//! `Infinity`, sparse arrays, or exotic objects, so most of the TS boundary
//! holds by construction. The surviving runtime check is negative zero:
//! `JSON.stringify(-0)` writes `"0"`, so a `-0` would silently change value
//! on the round trip and must be rejected like the TS `isJsonValue` does.

use serde_json::{Map, Number};

/// A value that round-trips losslessly through JSON.
pub use serde_json::Value as JsonValue;

/// Whether a JSON number survives a lossless round trip (rejects `-0`).
fn is_lossless_number(number: &Number) -> bool {
    match number.as_f64() {
        Some(value) => !(value == 0.0 && value.is_sign_negative()),
        None => true,
    }
}

/// Test the lossless JSON boundary without detaching (TS `isJsonValue`).
pub fn is_json_value(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
        JsonValue::Number(number) => is_lossless_number(number),
        JsonValue::Array(items) => items.iter().all(is_json_value),
        JsonValue::Object(record) => record.values().all(is_json_value),
    }
}

/// Validate and detach lossless JSON in one pass (TS `snapshotJsonValue`).
/// Returns the detached snapshot, or `None` when the value is not
/// losslessly JSON-serializable.
pub fn snapshot_json_value(value: &JsonValue) -> Option<JsonValue> {
    if is_json_value(value) {
        Some(value.clone())
    } else {
        None
    }
}

/// Deep structural equality over the session-event JSON value domain.
/// Numbers compare like JavaScript numbers (`===` on doubles, so `1` equals
/// `1.0` and `-0` equals `0`), replacing `node:util`'s
/// `isDeepStrictEqual` behavior on the JSON domain.
pub fn is_deep_equal_json(a: &JsonValue, b: &JsonValue) -> bool {
    match (a, b) {
        (JsonValue::Number(left), JsonValue::Number(right)) => {
            match (left.as_f64(), right.as_f64()) {
                (Some(left), Some(right)) => left == right,
                _ => left == right,
            }
        }
        _ => a == b,
    }
}

/// Whether one record has exactly the given keys and nothing else.
pub(crate) fn has_exact_keys(record: &Map<String, JsonValue>, keys: &[&str]) -> bool {
    record.len() == keys.len() && keys.iter().all(|key| record.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_plain_json_values() {
        assert!(is_json_value(&json!(null)));
        assert!(is_json_value(&json!(true)));
        assert!(is_json_value(&json!("text")));
        assert!(is_json_value(&json!(42)));
        assert!(is_json_value(&json!([1, "a", null])));
        assert!(is_json_value(&json!({"a": {"b": [1.5]}})));
    }

    #[test]
    fn rejects_negative_zero() {
        let negative_zero = JsonValue::Number(Number::from_f64(-0.0).unwrap());
        assert!(!is_json_value(&negative_zero));
        assert_eq!(snapshot_json_value(&negative_zero), None);
        let positive_zero = JsonValue::Number(Number::from_f64(0.0).unwrap());
        assert!(is_json_value(&positive_zero));
    }

    #[test]
    fn snapshot_detaches() {
        let value = json!({"nested": [1, 2]});
        let snapshot = snapshot_json_value(&value).unwrap();
        assert_eq!(snapshot, value);
        // Mutation of the original cannot reach the snapshot.
        let mut mutable = value;
        mutable["nested"][0] = json!(9);
        assert_eq!(snapshot, json!({"nested": [1, 2]}));
    }

    #[test]
    fn deep_equality() {
        assert!(is_deep_equal_json(&json!({"a": [1, 2]}), &json!({"a": [1, 2]})));
        assert!(!is_deep_equal_json(&json!({"a": [1, 2]}), &json!({"a": [2, 1]})));
        assert!(is_deep_equal_json(&json!(1), &json!(1.0)));
    }

    #[test]
    fn exact_keys() {
        let record = json!({"type": "x", "seq": 0, "time": 1, "data": null})
            .as_object()
            .unwrap()
            .clone();
        assert!(has_exact_keys(&record, &["type", "seq", "time", "data"]));
        assert!(!has_exact_keys(&record, &["type", "seq"]));
        assert!(!has_exact_keys(&record, &["type", "seq", "time", "data", "extra"]));
    }
}
