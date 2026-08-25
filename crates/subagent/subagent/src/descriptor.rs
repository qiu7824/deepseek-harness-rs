//! The durable subagent-child descriptor: the versioned, model-hidden
//! `subagent/descriptor` session event that identifies every session-backed
//! subagent and records whether it is one-shot or continuable. Rust port of
//! `packages/subagent/subagent/src/descriptor.ts`.

use dsh_session::SessionEvent;
use dsh_tools::ToolRestriction;
use serde_json::Value;

/// The current descriptor format version, stamped into every appended
/// `subagent/descriptor` event and required verbatim by
/// [`fold_subagent_descriptor`].
pub const SUBAGENT_DESCRIPTOR_VERSION: u32 = 3;

/// The supported durable subagent identity and optional continuation
/// composition (TS `SubagentDescriptorData`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum SubagentDescriptorData {
    /// A session-backed subagent that cannot be cold-resumed after its run.
    #[serde(rename = "one-shot")]
    OneShot {
        version: u32,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A session-backed subagent whose declared composition supports cold
    /// resume.
    Continuable {
        version: u32,
        provider: String,
        /// The initial delegation's short `description`, used for durable
        /// enumeration.
        label: String,
        /// Resolved child `agentOptions.provider`, when one was declared.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "agentProvider"
        )]
        agent_provider: Option<String>,
        /// Resolved child `agentOptions.model`, when one was declared.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "agentModel"
        )]
        agent_model: Option<String>,
        /// Resolved child reasoning effort, retained across cold resume.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "agentReasoningEffort"
        )]
        agent_reasoning_effort: Option<String>,
        /// Per-child persona that shadows the deployment persona on resume.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persona: Option<String>,
        /// Child tool scoping reapplied on resume.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "toolFilter"
        )]
        tool_filter: Option<ToolRestriction>,
    },
}

impl SubagentDescriptorData {
    pub fn provider(&self) -> &str {
        match self {
            SubagentDescriptorData::OneShot { provider, .. }
            | SubagentDescriptorData::Continuable { provider, .. } => provider,
        }
    }

    pub fn is_continuable(&self) -> bool {
        matches!(self, SubagentDescriptorData::Continuable { .. })
    }
}

const DESCRIPTOR_BASE_KEYS: [&str; 4] = ["version", "mode", "provider", "label"];
const TOOL_FILTER_KEYS: [&str; 2] = ["allow", "deny"];

/// Reject fields outside one versioned record's declared schema.
fn assert_known_keys(
    map: &serde_json::Map<String, Value>,
    keys: &[&str],
    path: &str,
) -> Result<(), String> {
    for key in map.keys() {
        if !keys.contains(&key.as_str()) {
            return Err(format!(
                "persisted subagent descriptor {path} has unknown field \"{key}\""
            ));
        }
    }
    Ok(())
}

/// Read one optional string field from a persisted descriptor record.
fn optional_string(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!(
            "persisted subagent descriptor {key} must be a string"
        )),
    }
}

/// Read one optional string-array field from a persisted tool restriction.
fn optional_string_array(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut strings = Vec::new();
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(format!(
                        "persisted subagent descriptor toolFilter.{key} must be an array of strings"
                    ));
                };
                strings.push(text.to_string());
            }
            Ok(Some(strings))
        }
        Some(_) => Err(format!(
            "persisted subagent descriptor toolFilter.{key} must be an array of strings"
        )),
    }
}

/// Validate and reconstruct a persisted tool restriction.
fn parse_tool_filter(value: &Value) -> Result<ToolRestriction, String> {
    let Some(map) = value.as_object() else {
        return Err("persisted subagent descriptor toolFilter must be an object".to_string());
    };
    assert_known_keys(map, &TOOL_FILTER_KEYS, "toolFilter")?;
    let allow = optional_string_array(map, "allow")?;
    let deny = optional_string_array(map, "deny")?;
    if allow.is_none() && deny.is_none() {
        return Err(
            "persisted subagent descriptor toolFilter must declare allow and/or deny".to_string(),
        );
    }
    Ok(ToolRestriction { allow, deny })
}

/// Validate one persisted descriptor payload for the current runtime.
fn parse_subagent_descriptor(value: &Value) -> Result<Option<SubagentDescriptorData>, String> {
    let Some(map) = value.as_object() else {
        return Err("persisted subagent descriptor payload must be an object".to_string());
    };
    let Some(version) = map.get("version").and_then(Value::as_u64) else {
        return Err("persisted subagent descriptor version must be a number".to_string());
    };
    if version != 2 && version as u32 != SUBAGENT_DESCRIPTOR_VERSION {
        return Ok(None);
    }
    let mode = map.get("mode").and_then(Value::as_str).unwrap_or("");
    if mode != "one-shot" && mode != "continuable" {
        return Err(
            "persisted subagent descriptor mode must be \"one-shot\" or \"continuable\""
                .to_string(),
        );
    }
    if mode == "one-shot" {
        assert_known_keys(map, &DESCRIPTOR_BASE_KEYS, "payload")?;
    } else {
        assert_known_keys(
            map,
            &[
                "version",
                "mode",
                "provider",
                "label",
                "agentProvider",
                "agentModel",
                "agentReasoningEffort",
                "persona",
                "toolFilter",
            ],
            "payload",
        )?;
    }
    let Some(provider) = map.get("provider").and_then(Value::as_str) else {
        return Err("persisted subagent descriptor provider must be a string".to_string());
    };
    if mode == "one-shot" {
        let label = optional_string(map, "label")?;
        return Ok(Some(SubagentDescriptorData::OneShot {
            version: SUBAGENT_DESCRIPTOR_VERSION,
            provider: provider.to_string(),
            label,
        }));
    }
    let Some(label) = map.get("label").and_then(Value::as_str) else {
        return Err("persisted subagent descriptor label must be a string".to_string());
    };
    let agent_provider = optional_string(map, "agentProvider")?;
    let agent_model = optional_string(map, "agentModel")?;
    let agent_reasoning_effort = optional_string(map, "agentReasoningEffort")?;
    let persona = optional_string(map, "persona")?;
    let tool_filter = match map.get("toolFilter") {
        None => None,
        Some(value) => Some(parse_tool_filter(value)?),
    };
    Ok(Some(SubagentDescriptorData::Continuable {
        version: SUBAGENT_DESCRIPTOR_VERSION,
        provider: provider.to_string(),
        label: label.to_string(),
        agent_provider,
        agent_model,
        agent_reasoning_effort,
        persona,
        tool_filter,
    }))
}

/// Validate and detach descriptor inputs into the durable payload, before
/// any Task or provider work begins.
pub fn snapshot_subagent_descriptor(
    input: &SubagentDescriptorData,
) -> Result<SubagentDescriptorData, String> {
    let candidate = match input {
        SubagentDescriptorData::OneShot {
            provider, label, ..
        } => SubagentDescriptorData::OneShot {
            version: SUBAGENT_DESCRIPTOR_VERSION,
            provider: provider.clone(),
            label: label.clone(),
        },
        SubagentDescriptorData::Continuable {
            provider,
            label,
            agent_provider,
            agent_model,
            agent_reasoning_effort,
            persona,
            tool_filter,
            ..
        } => SubagentDescriptorData::Continuable {
            version: SUBAGENT_DESCRIPTOR_VERSION,
            provider: provider.clone(),
            label: label.clone(),
            agent_provider: agent_provider.clone(),
            agent_model: agent_model.clone(),
            agent_reasoning_effort: agent_reasoning_effort.clone(),
            persona: persona.clone(),
            tool_filter: tool_filter.clone(),
        },
    };
    serde_json::to_value(&candidate)
        .map_err(|_| "subagent descriptor is not losslessly JSON-serializable".to_string())?;
    Ok(candidate)
}

/// Fold a persisted child log to its supported descriptor. The first
/// `subagent/descriptor` event is authoritative.
pub fn fold_subagent_descriptor(
    events: &[SessionEvent],
) -> Result<Option<SubagentDescriptorData>, String> {
    let Some(event) = events
        .iter()
        .find(|event| event.type_ == "subagent/descriptor")
    else {
        return Ok(None);
    };
    parse_subagent_descriptor(&event.data)
}
