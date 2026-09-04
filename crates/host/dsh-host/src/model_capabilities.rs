use indexmap::IndexMap;
use serde_json::Value;

/// Schemastery represents numbers as f64. Normalize only integral capacity
/// fields before serde's u64 conversion; fractions remain validation failures.
pub(crate) fn normalize_capacity_numbers(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if ["contextWindow", "maxTokens", "defaultContextWindow"].contains(&key.as_str()) {
                    if let Some(number) = value.as_f64().filter(|n| {
                        n.is_finite()
                            && *n >= 0.0
                            && n.fract() == 0.0
                            && *n <= 9_007_199_254_740_991.0
                    }) {
                        *value = serde_json::json!(number as u64);
                    }
                } else {
                    normalize_capacity_numbers(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_capacity_numbers(value);
            }
        }
        _ => {}
    }
}

/// Conservative fallbacks for documented model families. Endpoint metadata and
/// explicit settings take precedence; unfamiliar model names remain unknown.
pub(crate) fn inferred_efforts(model: &str) -> Option<IndexMap<String, Option<String>>> {
    let normalized = model.to_ascii_lowercase();
    if !normalized.is_ascii() {
        return None;
    }
    let mut id = normalized.rsplit('/').next()?;
    if id.len() > 11 {
        let suffix = &id[id.len() - 11..];
        if suffix.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 0 | 5 | 8) {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        }) {
            id = &id[..id.len() - 11];
        }
    }
    let levels: &[&str] = if ["claude-opus-4-6", "claude-sonnet-4-6"].contains(&id) {
        &["off", "low", "medium", "high", "max"]
    } else if id == "gpt-5-pro" {
        &["high"]
    } else if ["gpt-5.2-pro", "gpt-5.4-pro"].contains(&id) {
        &["medium", "high", "xhigh"]
    } else if ["gpt-5.1-codex", "gpt-5.1-codex-mini"].contains(&id) {
        &["low", "medium", "high"]
    } else if ["gpt-5.1-codex-max", "gpt-5.2-codex", "gpt-5.3-codex"].contains(&id) {
        &["low", "medium", "high", "xhigh"]
    } else if [
        "gpt-5.2",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.5",
    ]
    .contains(&id)
    {
        &["none", "low", "medium", "high", "xhigh"]
    } else if id == "gpt-5.1" {
        &["none", "low", "medium", "high"]
    } else if ["gpt-5", "gpt-5-mini", "gpt-5-nano"].contains(&id) {
        &["minimal", "low", "medium", "high"]
    } else if ["o1", "o3", "o3-mini", "o4-mini"].contains(&id) {
        &["low", "medium", "high"]
    } else {
        return None;
    };
    Some(
        levels
            .iter()
            .map(|wire| {
                (
                    if *wire == "none" {
                        "off"
                    } else if *wire == "xhigh" {
                        "max"
                    } else {
                        wire
                    }
                    .to_string(),
                    Some(wire.to_string()),
                )
            })
            .collect(),
    )
}

/// Accept only an actual level enumeration, never invent levels from a boolean
/// `reasoning: true` or `supported_parameters: [reasoning]` declaration.
pub(crate) fn discovered_efforts(entry: &Value) -> Option<Value> {
    if let Some(value) = entry
        .get("reasoningEfforts")
        .or_else(|| entry.get("reasoning_efforts"))
    {
        if value == &Value::Bool(false) {
            return Some(value.clone());
        }
        if let Some(map) = value.as_object() {
            if !map.is_empty()
                && map.iter().all(|(id, wire)| {
                    valid_level(id)
                        && (wire.is_null() && id == "off"
                            || wire.as_str().is_some_and(|wire| !wire.is_empty()))
                })
            {
                return Some(value.clone());
            }
        }
    }
    let values = entry
        .get("supported_reasoning_efforts")
        .or_else(|| entry.get("supported_reasoning_levels"))
        .or_else(|| entry.pointer("/capabilities/reasoning_efforts"))
        .or_else(|| entry.pointer("/reasoning/efforts"))?
        .as_array()?;
    let mut map = serde_json::Map::new();
    let has_max = values.iter().any(|value| {
        value.as_str().or_else(|| {
            value
                .get("id")
                .or_else(|| value.get("effort"))
                .and_then(Value::as_str)
        }) == Some("max")
    });
    for value in values {
        let wire = value.as_str().or_else(|| {
            value
                .get("id")
                .or_else(|| value.get("effort"))
                .and_then(Value::as_str)
        })?;
        let id = if wire == "none" {
            "off"
        } else if wire == "xhigh" && !has_max {
            "max"
        } else {
            wire
        };
        if !valid_level(id) {
            return None;
        }
        map.insert(id.to_string(), Value::String(wire.to_string()));
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

fn valid_level(id: &str) -> bool {
    [
        "off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn generations_and_pro_do_not_share_invalid_levels() {
        let original = inferred_efforts("openai/gpt-5").unwrap();
        assert!(!original.contains_key("off"));
        assert!(!original.contains_key("max"));
        let newer = inferred_efforts("gpt-5.4").unwrap();
        assert_eq!(newer["off"].as_deref(), Some("none"));
        assert!(!newer.contains_key("minimal"));
        assert_eq!(inferred_efforts("gpt-5-pro").unwrap().len(), 1);
        assert!(inferred_efforts("gpt-5.6-unknown").is_none());
        assert!(inferred_efforts("gpt-5.4-unknown").is_none());
        assert!(inferred_efforts("gpt-5.5-pro").is_none());
        assert!(
            !inferred_efforts("gpt-5.1-codex")
                .unwrap()
                .contains_key("max")
        );
        assert!(
            inferred_efforts("gpt-5.4-2026-03-05")
                .unwrap()
                .contains_key("max")
        );
    }
    #[test]
    fn discovery_requires_levels_and_preserves_wire_spelling() {
        assert!(discovered_efforts(&json!({"reasoning":true})).is_none());
        let levels =
            discovered_efforts(&json!({"supported_reasoning_efforts":["none", "low", "xhigh"]}))
                .unwrap();
        assert_eq!(levels["off"], "none");
        assert_eq!(levels["max"], "xhigh");
        let levels =
            discovered_efforts(&json!({"supported_reasoning_efforts":["high", "xhigh", "max"]}))
                .unwrap();
        assert_eq!(levels["xhigh"], "xhigh");
        assert_eq!(levels["max"], "max");
        assert_eq!(
            discovered_efforts(&json!({"reasoningEfforts":false})),
            Some(json!(false))
        );
    }
}
