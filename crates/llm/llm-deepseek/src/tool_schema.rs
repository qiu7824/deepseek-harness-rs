//! Tool parameter schemas for providers that require a plain object at the
//! schema root.
//!
//! Local tools may describe mutually exclusive argument sets with a root
//! `oneOf` of object branches. Claude (directly, or through Devin) rejects
//! `oneOf`, `anyOf` and `allOf` at the top level of `input_schema`, so the
//! request carries a single object instead: every branch property, the values
//! branches pin merged into an `enum`, the requirements shared by all
//! branches, and a short note naming each combination. Arguments are still
//! validated locally against the original schema before a tool runs.

use serde_json::{Map, Value, json};

const COMBINATORS: [&str; 3] = ["oneOf", "anyOf", "allOf"];

/// Values a branch restricts a property to, through `const` or `enum`.
fn pinned(property: &Value) -> Option<Vec<Value>> {
    if let Some(value) = property.get("const") {
        return Some(vec![value.clone()]);
    }
    property.get("enum").and_then(Value::as_array).cloned()
}

fn required(schema: &Value) -> Vec<String> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Returns `schema` unchanged unless its root carries a combinator.
pub(crate) fn object_root(schema: &Value) -> Value {
    let Some(root) = schema.as_object() else {
        return schema.clone();
    };
    if !COMBINATORS.iter().any(|key| root.contains_key(*key)) {
        return schema.clone();
    }
    let mut out = root.clone();
    let mut branches = Vec::new();
    for key in COMBINATORS {
        if let Some(Value::Array(list)) = out.remove(key) {
            branches.extend(list.into_iter().filter(Value::is_object));
        }
    }
    out.insert("type".into(), json!("object"));
    let mut properties = match out.remove("properties") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    // A property every branch pins gets the union of the pinned values.
    let mut unions: Vec<(String, Vec<Value>)> = Vec::new();
    for branch in &branches {
        for (name, property) in branch["properties"].as_object().into_iter().flatten() {
            if let Some(values) = pinned(property) {
                let union = match unions.iter_mut().find(|(key, _)| key == name) {
                    Some((_, union)) => union,
                    None => {
                        unions.push((name.clone(), Vec::new()));
                        &mut unions.last_mut().unwrap().1
                    }
                };
                for value in values {
                    if !union.contains(&value) {
                        union.push(value);
                    }
                }
            }
            properties.entry(name.clone()).or_insert_with(|| {
                let mut copy = property.clone();
                if let Some(map) = copy.as_object_mut() {
                    map.remove("const");
                    map.remove("enum");
                }
                copy
            });
        }
    }
    for (name, values) in &unions {
        let every_branch_pins = branches.iter().all(|branch| {
            branch
                .pointer(&format!("/properties/{name}"))
                .and_then(pinned)
                .is_some()
        });
        if let Some(Value::Object(property)) = properties.get_mut(name)
            && every_branch_pins
            && !property.contains_key("enum")
            && !property.contains_key("const")
        {
            property.insert("enum".into(), Value::Array(values.clone()));
        }
    }
    let mut shared: Vec<String> = required(&Value::Object(out.clone()));
    if let Some(first) = branches.first() {
        for name in required(first) {
            if branches
                .iter()
                .all(|branch| required(branch).contains(&name))
                && !shared.contains(&name)
            {
                shared.push(name);
            }
        }
    }
    out.insert("properties".into(), Value::Object(properties));
    if !shared.is_empty() {
        out.insert("required".into(), json!(shared));
    }
    let combinations: Vec<String> = branches
        .iter()
        .filter_map(|branch| {
            let fields = branch["properties"].as_object();
            let selectors: Vec<String> = fields
                .into_iter()
                .flatten()
                .filter_map(|(name, property)| {
                    let values = pinned(property)?;
                    let values: Vec<String> = values
                        .iter()
                        .map(|value| match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    Some(format!("{name}={}", values.join("|")))
                })
                .collect();
            let extra: Vec<String> = required(branch)
                .into_iter()
                .filter(|name| {
                    !shared.contains(name)
                        && fields
                            .and_then(|fields| fields.get(name))
                            .and_then(pinned)
                            .is_none()
                })
                .collect();
            if selectors.is_empty() && extra.is_empty() {
                return None;
            }
            let mut text = selectors.join(" ");
            if !extra.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&format!("requires {}", extra.join(", ")));
            }
            Some(text)
        })
        .collect();
    if !combinations.is_empty() {
        let note = format!("Valid combinations: {}.", combinations.join("; "));
        let description = match out.get("description").and_then(Value::as_str) {
            Some(text) if !text.trim().is_empty() => format!("{text}\n{note}"),
            _ => note,
        };
        out.insert("description".into(), json!(description));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::object_root;
    use serde_json::json;

    #[test]
    fn file_manage_branches_become_one_object() {
        // The file_manage schema as the Host registers it.
        let schema = json!({"type":"object","additionalProperties":false,"properties":{"action":{"type":"string","enum":["rename","delete"]},"file_path":{"type":"string","minLength":1,"maxLength":4096},"new_path":{"type":"string","minLength":1,"maxLength":4096}},"required":["action","file_path"],"oneOf":[{"type":"object","properties":{"action":{"type":"string","enum":["rename"]},"file_path":{},"new_path":{}},"required":["new_path"]},{"type":"object","properties":{"action":{"type":"string","enum":["delete"]},"file_path":{},"new_path":{}}}]});
        let adapted = object_root(&schema);
        assert!(adapted.get("oneOf").is_none());
        assert_eq!(adapted["type"], "object");
        assert_eq!(adapted["additionalProperties"], false);
        assert_eq!(adapted["required"], json!(["action", "file_path"]));
        assert_eq!(adapted["properties"], schema["properties"]);
        assert_eq!(
            adapted["description"],
            "Valid combinations: action=rename requires new_path; action=delete."
        );
        // The local schema keeps its branches for argument validation.
        assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn workspace_scratch_branches_keep_their_requirements_in_words() {
        let schema = json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["allocate","prepare_copy","inspect","promote","write","read","list","pin","release"]},
                "id":{"type":"string","minLength":1},
                "path":{"type":"string","minLength":1},
                "target":{"type":"string","minLength":1},
                "content":{"type":"string"},
                "expectedSha256":{"oneOf":[{"type":"string","minLength":64,"maxLength":64},{"type":"null"}]}
            },
            "required":["action"],"additionalProperties":false,
            "oneOf":[
                {"type":"object","properties":{"action":{"type":"string","enum":["allocate","prepare_copy","list"]}},"required":["action"]},
                {"type":"object","properties":{"action":{"type":"string","enum":["inspect"]},"target":{}},"required":["action","target"]},
                {"type":"object","properties":{"action":{"type":"string","enum":["promote"]},"id":{},"path":{},"target":{},"expectedSha256":{}},"required":["action","id","path","target","expectedSha256"]},
                {"type":"object","properties":{"action":{"type":"string","enum":["pin","release"]},"id":{}},"required":["action","id"]}
            ]
        });
        let adapted = object_root(&schema);
        assert!(adapted.get("oneOf").is_none());
        assert_eq!(adapted["required"], json!(["action"]));
        assert_eq!(adapted["properties"], schema["properties"]);
        // A nested oneOf below the root is accepted by Claude and kept.
        assert!(adapted["properties"]["expectedSha256"]["oneOf"].is_array());
        assert_eq!(
            adapted["description"],
            "Valid combinations: action=allocate|prepare_copy|list; action=inspect requires target; action=promote requires id, path, target, expectedSha256; action=pin|release requires id."
        );
    }

    #[test]
    fn properties_declared_only_in_branches_are_carried_to_the_root() {
        let schema = json!({
            "type":"object","description":"Manage files.",
            "oneOf":[
                {"type":"object","properties":{"op":{"const":"move"},"from":{"type":"string"},"to":{"type":"string"}},"required":["op","from","to"]},
                {"type":"object","properties":{"op":{"const":"delete"},"path":{"type":"string"}},"required":["op","path"]}
            ]
        });
        let adapted = object_root(&schema);
        let properties = adapted["properties"].as_object().unwrap();
        for name in ["op", "from", "to", "path"] {
            assert!(properties.contains_key(name), "{name} missing");
        }
        assert_eq!(
            adapted["properties"]["op"]["enum"],
            json!(["move", "delete"])
        );
        assert_eq!(adapted["required"], json!(["op"]));
        assert_eq!(
            adapted["description"],
            "Manage files.\nValid combinations: op=move requires from, to; op=delete requires path."
        );
    }

    #[test]
    fn nested_combinators_and_plain_roots_are_left_alone() {
        let plain = json!({"type":"object","properties":{"id":{"oneOf":[{"type":"string"},{"type":"null"}]}}});
        assert_eq!(object_root(&plain), plain);
        assert_eq!(object_root(&json!(true)), json!(true));
    }
}
