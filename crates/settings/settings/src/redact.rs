//! Structural secret redaction for settings values. Rust port of
//! `packages/settings/settings/src/redact.ts`.
//!
//! # Deviation
//!
//! - The walker follows `object`, `dict`, and `array` containers exactly as
//!   TS; a secret buried inside a union branch or transform is returned
//!   verbatim (the TS TODO fail-closed note applies identically).

use schemastery::{Data, Node, Schema};

/// One schema-declared secret position inside a redacted value.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactedSecret {
    /// Path from the section root to the removed field.
    pub path: Vec<String>,
    /// Whether the field held a value before redaction.
    pub set: bool,
}

/// A value with every `role('secret')` field removed, plus the removal
/// record.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactedValue {
    /// Detached copy of the input with secret fields absent.
    pub value: Data,
    /// Every reachable secret position.
    pub secrets: Vec<RedactedSecret>,
}

fn walk(
    node: Option<&Schema>,
    value: &Data,
    path: &[String],
    secrets: &mut Vec<RedactedSecret>,
) -> Data {
    let Some(node) = node else {
        return value.clone();
    };
    if node.meta().role.as_deref() == Some("secret") {
        secrets.push(RedactedSecret {
            path: path.to_vec(),
            set: !value.is_nullish(),
        });
        return Data::Undefined;
    }
    match node.node() {
        Node::Object(properties) => {
            let mut rebuilt = indexmap::IndexMap::new();
            let source = match value {
                Data::Object(object) => Some(object),
                _ => None,
            };
            if let Some(source) = source {
                for (key, entry) in source {
                    if properties.contains_key(key) {
                        continue;
                    }
                    rebuilt.insert(key.clone(), entry.clone());
                }
            }
            for (key, child) in properties {
                let child_value = source
                    .and_then(|source| source.get(key))
                    .cloned()
                    .unwrap_or(Data::Undefined);
                let mut path = path.to_vec();
                path.push(key.clone());
                let stripped = walk(Some(child), &child_value, &path, secrets);
                if !stripped.is_nullish() {
                    rebuilt.insert(key.clone(), stripped);
                }
            }
            if source.is_none() && rebuilt.is_empty() {
                value.clone()
            } else {
                Data::Object(rebuilt)
            }
        }
        Node::Dict { inner, .. } => {
            let Data::Object(object) = value else {
                return value.clone();
            };
            let mut rebuilt = indexmap::IndexMap::new();
            for (key, entry) in object {
                let mut path = path.to_vec();
                path.push(key.clone());
                let stripped = walk(Some(inner), entry, &path, secrets);
                if !stripped.is_nullish() {
                    rebuilt.insert(key.clone(), stripped);
                }
            }
            Data::Object(rebuilt)
        }
        Node::Array(inner) => {
            let Data::Array(array) = value else {
                return value.clone();
            };
            let rebuilt = array
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let mut path = path.to_vec();
                    path.push(index.to_string());
                    walk(Some(inner), entry, &path, secrets)
                })
                .collect();
            Data::Array(rebuilt)
        }
        _ => value.clone(),
    }
}

/// Remove every `role('secret')` field a schema declares from a value (TS
/// `redactSecrets`). The input is never mutated.
pub fn redact_secrets(schema: &Schema, value: &Data) -> RedactedValue {
    let mut secrets = Vec::new();
    let stripped = walk(Some(schema), value, &[], &mut secrets);
    RedactedValue {
        value: stripped,
        secrets,
    }
}
