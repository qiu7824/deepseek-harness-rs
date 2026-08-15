//! Validation data model: JSON values plus `Date`/`RegExp`/binary/instance
//! variants (mirrors the JS value space schemastery operates on).

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

/// A validated input value. `Null` and `Undefined` are both "nullish"
/// (TS `null`/`undefined`).
#[derive(Clone)]
pub enum Data {
    Null,
    Undefined,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Data>),
    Object(IndexMap<String, Data>),
    Date(DateTime<Utc>),
    RegExp { source: String, flags: String },
    Binary(Vec<u8>),
    /// Class/function instance carrying its constructor name
    /// (TS `instanceof`/prototype-chain checks are reduced to name checks).
    Instance { name: &'static str, value: Arc<dyn std::any::Any + Send + Sync> },
}

impl Data {
    /// Build a named instance value (`Schema.is(name)` checks this name).
    pub fn instance<T: std::any::Any + Send + Sync>(
        name: &'static str,
        value: T,
    ) -> Self {
        Data::Instance { name, value: Arc::new(value) }
    }

    /// Constructor name of an instance value, if any.
    pub fn instance_name(&self) -> Option<&'static str> {
        match self {
            Data::Instance { name, .. } => Some(*name),
            _ => None,
        }
    }
    /// JSON `null` (explicit null).
    pub fn null() -> Self {
        Data::Null
    }

    /// JS `undefined` (absent property or unset default).
    pub fn undefined() -> Self {
        Data::Undefined
    }

    /// TS `isNullable`: `null` or `undefined`.
    pub fn is_nullish(&self) -> bool {
        matches!(self, Data::Null | Data::Undefined)
    }

    /// Whether this value is a JSON object (non-array).
    pub fn is_object(&self) -> bool {
        matches!(self, Data::Object(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Data::Array(_))
    }

    /// Mutable member access (JS `data[key]`); `None` when absent.
    pub fn member_mut(&mut self, key: &PathKey) -> Option<&mut Data> {
        match (self, key) {
            (Data::Object(map), PathKey::Key(key)) => map.get_mut(key),
            (Data::Array(list), PathKey::Index(index)) => list.get_mut(*index),
            _ => None,
        }
    }

    /// Set or replace a member (JS `data[key] = value`).
    pub fn set_member(&mut self, key: PathKey, value: Data) {
        match (self, key) {
            (Data::Object(map), PathKey::Key(key)) => {
                map.insert(key, value);
            }
            (Data::Array(list), PathKey::Index(index)) => {
                if index >= list.len() {
                    list.resize(index + 1, Data::Undefined);
                }
                list[index] = value;
            }
            _ => {}
        }
    }

    /// Remove a member (JS `delete data[key]`).
    pub fn remove_member(&mut self, key: &PathKey) {
        match (self, key) {
            (Data::Object(map), PathKey::Key(key)) => {
                map.shift_remove(key);
            }
            (Data::Array(list), PathKey::Index(index)) => {
                if *index < list.len() {
                    list[*index] = Data::Undefined;
                }
            }
            _ => {}
        }
    }

    /// Whether a member exists (JS `key in data`).
    pub fn has_member(&self, key: &PathKey) -> bool {
        match (self, key) {
            (Data::Object(map), PathKey::Key(key)) => map.contains_key(key),
            (Data::Array(list), PathKey::Index(index)) => *index < list.len(),
            _ => false,
        }
    }

    /// Interpret a value as a number when possible (`parseFloat` semantics
    /// are handled at call sites; this is the plain f64 view).
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Data::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// Convert to a `serde_json::Value` when JSON-compatible.
    pub fn to_json(&self) -> Option<serde_json::Value> {
        Some(match self {
            Data::Null | Data::Undefined => serde_json::Value::Null,
            Data::Bool(value) => serde_json::Value::Bool(*value),
            Data::Number(value) => serde_json::Number::from_f64(*value)?.into(),
            Data::String(value) => serde_json::Value::String(value.clone()),
            Data::Array(list) => serde_json::Value::Array(
                list.iter().map(|item| item.to_json()).collect::<Option<_>>()?,
            ),
            Data::Object(map) => {
                let mut object = serde_json::Map::new();
                for (key, value) in map {
                    object.insert(key.clone(), value.to_json()?);
                }
                serde_json::Value::Object(object)
            }
            Data::Date(_) | Data::RegExp { .. } | Data::Binary(_) | Data::Instance { .. } => {
                return None
            }
        })
    }

    /// JS `JSON.stringify` approximation used in union/intersect errors.
    pub fn to_json_string(&self) -> String {
        match self {
            Data::Date(date) => serde_json::Value::String(date.to_rfc3339()).to_string(),
            Data::RegExp { .. } | Data::Binary(_) | Data::Instance { .. } => "{}".to_string(),
            _ => self
                .to_json()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string()),
        }
    }

    /// JS `String(value)` approximation used in validation messages.
    pub fn to_js_string(&self) -> String {
        match self {
            Data::Null => "null".to_string(),
            Data::Undefined => "undefined".to_string(),
            Data::Bool(value) => value.to_string(),
            Data::Number(value) => js_number_string(*value),
            Data::String(value) => value.clone(),
            Data::Array(list) => list
                .iter()
                .map(|item| item.to_js_string())
                .collect::<Vec<_>>()
                .join(","),
            Data::Date(date) => date.to_rfc3339(),
            Data::RegExp { source, flags } => format!("/{source}/{flags}"),
            Data::Binary(_) => "[object ArrayBuffer]".to_string(),
            Data::Instance { name, .. } => format!("[object {name}]"),
            Data::Object(_) => "[object Object]".to_string(),
        }
    }

    /// Deep equality following the TS `deepEqual` semantics relevant to
    /// validation (`strict` distinguishes null-vs-undefined handling).
    pub fn deep_equal(a: &Data, b: &Data, strict: bool) -> bool {
        if std::ptr::eq(a, b) {
            return true;
        }
        if !strict && a.is_nullish() && b.is_nullish() {
            return true;
        }
        match (a, b) {
            (Data::Null, Data::Null) | (Data::Undefined, Data::Undefined) => true,
            (Data::Bool(x), Data::Bool(y)) => x == y,
            (Data::Number(x), Data::Number(y)) => x == y,
            (Data::String(x), Data::String(y)) => x == y,
            (Data::Array(x), Data::Array(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .zip(y.iter())
                        .all(|(item_a, item_b)| Data::deep_equal(item_a, item_b, strict))
            }
            (Data::Object(x), Data::Object(y)) => {
                let keys: std::collections::HashSet<&String> =
                    x.keys().chain(y.keys()).collect();
                keys.into_iter().all(|key| match (x.get(key), y.get(key)) {
                    (Some(value_a), Some(value_b)) => {
                        Data::deep_equal(value_a, value_b, strict)
                    }
                    _ => false,
                })
            }
            (Data::Date(x), Data::Date(y)) => x == y,
            (Data::RegExp { source: sx, flags: fx }, Data::RegExp { source: sy, flags: fy }) => {
                sx == sy && fx == fy
            }
            (Data::Binary(x), Data::Binary(y)) => x == y,
            (
                Data::Instance { name: nx, value: x },
                Data::Instance { name: ny, value: y },
            ) => nx == ny && Arc::ptr_eq(x, y),
            _ => false,
        }
    }
}

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Data::Null, Data::Null) | (Data::Undefined, Data::Undefined) => true,
            (Data::Bool(a), Data::Bool(b)) => a == b,
            (Data::Number(a), Data::Number(b)) => a == b,
            (Data::String(a), Data::String(b)) => a == b,
            (Data::Array(a), Data::Array(b)) => a == b,
            (Data::Object(a), Data::Object(b)) => a == b,
            (Data::Date(a), Data::Date(b)) => a == b,
            (
                Data::RegExp { source: sa, flags: fa },
                Data::RegExp { source: sb, flags: fb },
            ) => sa == sb && fa == fb,
            (Data::Binary(a), Data::Binary(b)) => a == b,
            (
                Data::Instance { name: na, value: va },
                Data::Instance { name: nb, value: vb },
            ) => na == nb && Arc::ptr_eq(va, vb),
            _ => false,
        }
    }
}

/// JS `String(number)` formatting (`NaN`/`Infinity`/no trailing `.0`).
pub fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    if value == 0.0 {
        return "0".to_string();
    }
    format!("{value}")
}

/// Object key or array index used by path formatting and member access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathKey {
    Key(String),
    Index(usize),
}

impl fmt::Display for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_js_string())
    }
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Data::Instance { name, .. } => write!(f, "Instance({name})"),
            _ => write!(f, "{self}"),
        }
    }
}
