use super::V3Dialect;
use serde_json::{Map, Value};

pub(super) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Decode the physical framing before the adjacent logical transform. Rust V3
/// retained the older seedLength header; released V3 records isSeeded and puts
/// the exact cut in the accepted end-seed marker.
pub fn decode_v3_header(
    mut physical: Value,
    dialect: V3Dialect,
) -> Result<(Value, Option<u64>), String> {
    let data = physical
        .as_object_mut()
        .ok_or("physical Session header must be an object")?;
    if data.remove("type") != Some(Value::String("session".into())) {
        return Err("physical Session header requires type session".into());
    }
    let inherited = if dialect == V3Dialect::Rust && !data.contains_key("isSeeded") {
        let seed = data
            .remove("seedLength")
            .map(|v| count(&v, "seedLength"))
            .transpose()?;
        data.insert("isSeeded".into(), Value::Bool(seed.is_some()));
        Some(seed.unwrap_or(0))
    } else {
        if data.contains_key("seedLength") {
            return Err("seedLength is not part of released V3 framing".into());
        }
        if data.get("isSeeded") == Some(&Value::Bool(false)) {
            Some(0)
        } else {
            None
        }
    };
    if dialect == V3Dialect::Rust && !data.contains_key("delegationDepth") {
        data.insert("delegationDepth".into(), Value::from(0));
    }
    validate_header(&physical, 3)?;
    for key in ["version", "createdAt", "delegationDepth"] {
        physical[key] = Value::from(count(&physical[key], key)?);
    }
    Ok((physical, inherited))
}

pub(super) fn count(value: &Value, field: &str) -> Result<u64, String> {
    let number = value
        .as_u64()
        .filter(|n| *n <= MAX_SAFE_INTEGER)
        .or_else(|| {
            value
                .as_f64()
                .filter(|n| {
                    n.is_finite()
                        && !n.is_sign_negative()
                        && *n <= MAX_SAFE_INTEGER as f64
                        && n.fract() == 0.0
                })
                .map(|n| n as u64)
        });
    number.ok_or_else(|| format!("{field} must be a nonnegative safe integer"))
}

pub(super) fn object<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{field} must be an object"))
}

pub(super) fn validate_header(header: &Value, version: u64) -> Result<(), String> {
    let header = object(header, "Session header")?;
    for key in ["version", "id", "createdAt", "isSeeded", "delegationDepth"] {
        if !header.contains_key(key) {
            return Err(format!("Session header lacks {key}"));
        }
    }
    for key in header.keys() {
        if ![
            "version",
            "id",
            "createdAt",
            "isSeeded",
            "delegationDepth",
            "cwd",
            "parentSession",
            "origin",
            "agentPreset",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("Session header has unknown field {key}"));
        }
    }
    if count(&header["version"], "version")? != version {
        return Err(format!("expected V{version} header"));
    }
    if !header["id"].is_string() || !header["isSeeded"].is_boolean() {
        return Err("invalid Session identity or isSeeded".into());
    }
    count(&header["createdAt"], "createdAt")?;
    count(&header["delegationDepth"], "delegationDepth")?;
    for key in ["parentSession", "agentPreset"] {
        if header.get(key).is_some_and(|v| !v.is_string()) {
            return Err(format!("{key} must be a string"));
        }
    }
    if header.get("origin").is_some_and(|v| v != "subagent") {
        return Err("Session origin must be subagent when present".into());
    }
    if let Some(value) = header.get("cwd") {
        let path = value.as_str().ok_or("cwd must be a string")?;
        let absolute = path.starts_with('/')
            || path.starts_with('\\')
            || (path.as_bytes().get(1) == Some(&b':')
                && path
                    .as_bytes()
                    .get(2)
                    .is_some_and(|c| *c == b'/' || *c == b'\\'));
        if !absolute {
            return Err("cwd must be absolute".into());
        }
    }
    Ok(())
}
