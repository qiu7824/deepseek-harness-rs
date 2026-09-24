use super::wire::{count, object};
use serde_json::{Value, json};

pub(super) fn validate_catalog(value: &Value) -> Result<(), String> {
    let data = object(value, "subagent/catalog")?;
    let version = count(
        data.get("version").unwrap_or(&Value::Null),
        "catalog version",
    )?;
    let mode = data.get("mode").and_then(Value::as_str);
    if ![0, 1].contains(&version)
        || !data.get("childId").is_some_and(Value::is_string)
        || !matches!(mode, Some("continuable" | "one-shot" | "unknown"))
        || version == 0 && mode == Some("unknown")
        || mode == Some("continuable") && !data.get("label").is_some_and(Value::is_string)
        || data.get("label").is_some_and(|v| !v.is_string())
    {
        return Err("subagent/catalog requires a supported versioned discovery fact".into());
    }
    count(
        data.get("childCreatedAt").unwrap_or(&Value::Null),
        "childCreatedAt",
    )?;
    Ok(())
}

pub(super) fn validate_source(value: &Value) -> Result<(), String> {
    let source = object(value, "historical child evidence")?;
    if !source.get("childId").is_some_and(Value::is_string) || !source.contains_key("descriptor") {
        return Err("historical child identity and descriptor evidence are required".into());
    }
    count(&value["childCreatedAt"], "childCreatedAt")?;
    count(&value["descriptorCount"], "descriptorCount")?;
    Ok(())
}

pub(super) fn child_fact(source: &Value) -> Result<Option<Value>, String> {
    validate_source(source)?;
    let descriptor = &source["descriptor"];
    let version = count(&descriptor["version"], "descriptor version").ok();
    if count(&source["descriptorCount"], "descriptorCount")? != 1 || !matches!(version, Some(1..=5))
    {
        return Ok(None);
    }
    if !descriptor["provider"].is_string() {
        return Err(format!(
            "child {} has invalid descriptor provider",
            source["childId"]
        ));
    }
    let mode = if version == Some(1) {
        "continuable"
    } else {
        descriptor["mode"]
            .as_str()
            .filter(|v| ["continuable", "one-shot"].contains(v))
            .ok_or("invalid historical descriptor mode")?
    };
    let mut fact = json!({"version":0,"childId":source["childId"],"childCreatedAt":source["childCreatedAt"],"mode":mode});
    if let Some(label) = descriptor.get("label") {
        fact["label"] = label.clone();
    }
    validate_catalog(&fact)?;
    Ok(Some(fact))
}

/// Collect only a child's own descriptors after its exact inherited cut.
/// Persistence must supply the complete available direct-child set; filenames
/// and tool arguments never substitute for a child's recorded identity.
pub fn historical_child_catalog_source<'a>(
    header: &Value,
    inherited: u64,
    events: impl IntoIterator<Item = &'a Value>,
) -> Result<Value, String> {
    if header["origin"] != "subagent"
        || !header["parentSession"].is_string()
        || !header["id"].is_string()
    {
        return Err("catalog discovery requires a recorded direct subagent child".into());
    }
    let mut first = Value::Null;
    let mut descriptors = 0_u64;
    let mut event_count = 0;
    for event in events {
        let seq = count(&event["seq"], "child event seq")?;
        if seq != event_count {
            return Err("child events must be dense".into());
        }
        event_count += 1;
        if seq >= inherited && event["type"] == "subagent/descriptor" {
            if descriptors == 0 {
                first = event["data"].clone();
            }
            descriptors += 1;
        }
    }
    if inherited > event_count {
        return Err("child inherited cut exceeds its events".into());
    }
    let source = json!({"childId":header["id"],"childCreatedAt":header["createdAt"],"descriptorCount":descriptors,"descriptor":first});
    child_fact(&source)?;
    Ok(source)
}
