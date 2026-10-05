//! Preserve tool constraints while avoiding Devin's rejected root compositions.
use serde_json::{Map, Value, json};

const COMPOSITIONS: [&str; 3] = ["oneOf", "anyOf", "allOf"];

/// Devin rejects object-root compositions even when their JSON Schema is valid.
/// Double negation keeps their validation result without changing nested unions,
/// the host's canonical schema, or its argument validation and approval rules.
pub(super) fn for_wire(parameters: &Value) -> Value {
    let mut wire = parameters.clone();
    if parameters.get("type").and_then(Value::as_str) != Some("object")
        || parameters.get("not").is_some()
        || !COMPOSITIONS.iter().any(|key| parameters.get(key).is_some())
        || has_relocation_sensitive_schema(parameters)
    {
        return wire;
    }

    let object = wire.as_object_mut().expect("object schema with a type");
    let mut constraints = Map::new();
    for key in COMPOSITIONS {
        if let Some(value) = object.remove(key) {
            constraints.insert(key.into(), value);
        }
    }
    object.insert("not".into(), json!({"not": constraints}));
    wire
}

fn has_relocation_sensitive_schema(root: &Value) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let Some(object) = node.as_object() else {
            continue;
        };
        // Moving nodes can change reference targets or resource scope. Negation
        // also discards evaluated-property/item annotations, so do not rewrite
        // schemas whose validation can consume those annotations.
        if [
            "$ref",
            "$id",
            "id",
            "$anchor",
            "$dynamicRef",
            "$dynamicAnchor",
            "$recursiveRef",
            "$recursiveAnchor",
            "unevaluatedProperties",
            "unevaluatedItems",
        ]
        .iter()
        .any(|key| object.contains_key(*key))
        {
            return true;
        }
        for key in [
            "properties",
            "patternProperties",
            "$defs",
            "definitions",
            "dependentSchemas",
            "dependencies",
        ] {
            if let Some(children) = object.get(key).and_then(Value::as_object) {
                pending.extend(children.values());
            }
        }
        for key in [
            "items",
            "additionalItems",
            "additionalProperties",
            "contains",
            "propertyNames",
            "if",
            "then",
            "else",
            "not",
            "contentSchema",
            "oneOf",
            "anyOf",
            "allOf",
            "prefixItems",
        ] {
            if let Some(child) = object.get(key) {
                if let Some(children) = child.as_array() {
                    pending.extend(children);
                } else {
                    pending.push(child);
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_schema() -> Value {
        json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["inspect","promote"]},
                "target":{"type":"string","minLength":1},
                "expectedSha256":{"oneOf":[{"type":"string","minLength":64,"maxLength":64},{"type":"null"}]}
            },
            "required":["action"],"additionalProperties":false,
            "oneOf":[
                {"type":"object","properties":{"action":{"type":"string","enum":["inspect"]},"target":{}},"required":["action","target"]},
                {"type":"object","properties":{"action":{"type":"string","enum":["promote"]},"target":{},"expectedSha256":{}},"required":["action","target","expectedSha256"]}
            ]
        })
    }

    #[test]
    fn scratch_conditions_and_nullable_hash_remain_intact() {
        let original = scratch_schema();
        let snapshot = original.clone();
        let wire = for_wire(&original);
        assert_eq!(original, snapshot);
        assert!(wire.get("oneOf").is_none());
        assert_eq!(wire["not"]["not"]["oneOf"], original["oneOf"]);
        assert_eq!(wire["properties"], original["properties"]);
        let mut restored = wire;
        restored.as_object_mut().unwrap().remove("not");
        restored["oneOf"] = original["oneOf"].clone();
        assert_eq!(restored, original);
    }

    #[test]
    fn file_manage_rename_requirement_is_not_removed() {
        let original = json!({
            "type":"object","additionalProperties":false,
            "properties":{"action":{"type":"string","enum":["rename","delete"]},"file_path":{"type":"string"},"new_path":{"type":"string"}},
            "required":["action","file_path"],
            "oneOf":[
                {"type":"object","properties":{"action":{"type":"string","enum":["rename"]},"file_path":{},"new_path":{}},"required":["new_path"]},
                {"type":"object","properties":{"action":{"type":"string","enum":["delete"]},"file_path":{},"new_path":{}}}
            ]
        });
        let wire = for_wire(&original);
        assert_eq!(wire["not"]["not"]["oneOf"], original["oneOf"]);
        assert_eq!(wire["required"], original["required"]);
        assert_eq!(wire["additionalProperties"], false);
    }

    #[test]
    fn multiple_root_compositions_keep_their_conjunction() {
        let original = json!({"type":"object","oneOf":[{"required":["a"]},{"required":["b"]}],"anyOf":[{"required":["c"]},{"required":["d"]}],"allOf":[{"required":["e"]}],"description":"Keep all conditions"});
        let wire = for_wire(&original);
        for key in COMPOSITIONS {
            assert!(wire.get(key).is_none());
            assert_eq!(wire["not"]["not"][key], original[key]);
        }
        assert_eq!(wire["description"], original["description"]);
    }

    #[test]
    fn non_object_roots_nested_compositions_and_existing_not_are_unchanged() {
        for original in [
            json!({"oneOf":[{"type":"object"},{"type":"null"}]}),
            json!({"type":"string","anyOf":[{"const":"a"},{"const":"b"}]}),
            json!({"type":["object","null"],"allOf":[{"required":["a"]}]}),
            json!({"type":"object","properties":{"value":{"oneOf":[{"type":"string"},{"type":"null"}]}}}),
            json!({"type":"object","oneOf":[{"required":["a"]},{"required":["b"]}],"not":{"required":["forbidden"]}}),
        ] {
            assert_eq!(for_wire(&original), original);
        }
    }

    #[test]
    fn references_scopes_and_evaluation_annotations_prevent_relocation() {
        for key in [
            "$ref",
            "$id",
            "id",
            "$anchor",
            "$dynamicRef",
            "$dynamicAnchor",
            "$recursiveRef",
            "$recursiveAnchor",
            "unevaluatedProperties",
            "unevaluatedItems",
        ] {
            for location in ["root", "property", "branch", "definition", "items"] {
                let mut original = scratch_schema();
                let sensitive = json!({key: "sensitive"});
                match location {
                    "root" => original[key] = sensitive[key].clone(),
                    "property" => original["properties"]["sensitive"] = sensitive,
                    "branch" => original["oneOf"][0]["properties"]["sensitive"] = sensitive,
                    "definition" => original["$defs"] = json!({"Sensitive":sensitive}),
                    "items" => {
                        original["properties"]["values"] = json!({"type":"array","items":sensitive})
                    }
                    _ => unreachable!(),
                }
                assert_eq!(for_wire(&original), original, "{key} at {location}");
            }
        }
    }

    #[test]
    fn instance_data_and_property_names_are_not_treated_as_schema_keywords() {
        let mut original = scratch_schema();
        let data =
            json!({"$ref":"literal","$id":"literal","unevaluatedProperties":false,"oneOf":[{}]});
        original["default"] = data.clone();
        original["examples"] = json!([data]);
        original["properties"]["payload"] = json!({"const":data,"enum":[data]});
        original["properties"]["$ref"] = json!({"type":"string"});
        let wire = for_wire(&original);
        assert!(wire.get("oneOf").is_none());
        assert_eq!(wire["not"]["not"]["oneOf"], original["oneOf"]);
        for key in ["default", "examples", "properties"] {
            assert_eq!(wire[key], original[key]);
        }
    }
}
