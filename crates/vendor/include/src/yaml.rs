//! YAML ↔ JSON conversion with the `!!js` expression dialect
//! (port of the `JsExpr` js-yaml type and the surrounding read/write paths).
//!
//! Parsing uses `saphyr` (0.0.11, tag-preserving) because serde_yaml 0.9
//! silently drops unknown tags (`!!js` scalars collapse to plain strings);
//! dumps go through serde_yaml, which round-trips its own `Tagged` values.

use serde_json::{Map, Number, Value};

/// JSON node shape produced by the `!!js` YAML scalar type.
pub fn js_expr_object(expr: &str) -> Value {
    let mut map = Map::new();
    map.insert("__jsExpr".to_string(), Value::String(expr.to_string()));
    Value::Object(map)
}

fn is_js_tag(handle: &str, suffix: &str) -> bool {
    // `!!js` resolves to the core-schema handle + `js` suffix; the long
    // `!<tag:yaml.org,2002:js>` form keeps an empty handle.
    (suffix == "js" && (handle == "!!" || handle == "tag:yaml.org,2002:"))
        || (handle.is_empty() && suffix == "tag:yaml.org,2002:js")
}

/// Parse YAML text into JSON, translating `!!js` scalars into
/// `{ "__jsExpr": ... }` nodes and stringifying non-string mapping keys.
///
/// The loader runs with `early_parse(false)` so tagged scalars keep their
/// `Representation` (saphyr drops scalar tags during early parse); untagged
/// scalars are resolved through saphyr's own core-schema parser.
pub fn parse_yaml(text: &str) -> Result<Value, String> {
    let mut loader = saphyr::YamlLoader::<saphyr::Yaml>::default();
    loader.early_parse(false);
    let mut parser = saphyr_parser::Parser::new_from_str(text);
    parser
        .load(&mut loader, true)
        .map_err(|error| error.to_string())?;
    let docs = loader.into_documents();
    let doc = docs
        .into_iter()
        .next()
        .ok_or_else(|| "empty YAML document".to_string())?;
    yaml_to_json(&doc)
}

fn scalar_to_json(scalar: &saphyr::Scalar) -> Result<Value, String> {
    use saphyr::Scalar;
    match scalar {
        Scalar::Null => Ok(Value::Null),
        Scalar::Boolean(bool) => Ok(Value::Bool(*bool)),
        Scalar::Integer(int) => Ok(Value::Number((*int).into())),
        Scalar::FloatingPoint(float) => Number::from_f64(float.0)
            .map(Value::Number)
            .ok_or_else(|| format!("unsupported YAML float {}", float.0)),
        Scalar::String(text) => Ok(Value::String(text.to_string())),
    }
}

/// Convert a parsed YAML value into JSON.
pub fn yaml_to_json(value: &saphyr::Yaml) -> Result<Value, String> {
    use saphyr::Yaml;
    match value {
        Yaml::Value(scalar) => scalar_to_json(scalar),
        // Tagged scalars arrive as `Representation` with the tag in the
        // third field (early_parse(false)); untagged scalars resolve through
        // saphyr's core-schema parser.
        Yaml::Representation(text, style, tag) => {
            if let Some(tag) = tag
                && is_js_tag(&tag.handle, &tag.suffix)
            {
                return Ok(js_expr_object(text));
            }
            let scalar = saphyr::Scalar::parse_from_cow_and_metadata(
                std::borrow::Cow::Borrowed(text.as_ref()),
                *style,
                tag.as_ref(),
            )
            .ok_or_else(|| format!("invalid scalar representation {text}"))?;
            scalar_to_json(&scalar)
        }
        Yaml::Sequence(list) => {
            let mut result = Vec::with_capacity(list.len());
            for item in list {
                result.push(yaml_to_json(item)?);
            }
            Ok(Value::Array(result))
        }
        Yaml::Mapping(map) => {
            let mut result = Map::new();
            for (key, item) in map {
                let key = yaml_key_to_string(key)?;
                result.insert(key, yaml_to_json(item)?);
            }
            Ok(Value::Object(result))
        }
        Yaml::Tagged(tag, inner) => {
            if is_js_tag(&tag.handle, &tag.suffix) {
                let expr = match inner.as_ref() {
                    Yaml::Value(saphyr::Scalar::String(text)) => text.to_string(),
                    Yaml::Representation(text, _, _) => text.to_string(),
                    _ => return Err("!!js value must be a string scalar".to_string()),
                };
                Ok(js_expr_object(&expr))
            } else {
                yaml_to_json(inner)
            }
        }
        Yaml::Alias(_) => Err("YAML aliases are not supported".to_string()),
        Yaml::BadValue => Err("invalid YAML value".to_string()),
    }
}

fn yaml_key_to_string(key: &saphyr::Yaml) -> Result<String, String> {
    use saphyr::{Scalar, Yaml};
    match key {
        Yaml::Value(Scalar::String(text)) => Ok(text.to_string()),
        Yaml::Value(Scalar::Null) => Ok("null".to_string()),
        Yaml::Value(Scalar::Boolean(bool)) => Ok(bool.to_string()),
        Yaml::Value(Scalar::Integer(int)) => Ok(int.to_string()),
        Yaml::Value(Scalar::FloatingPoint(float)) => Ok(format!("{}", float.0)),
        Yaml::Representation(text, _, _) => Ok(text.to_string()),
        Yaml::Tagged(_, inner) => yaml_key_to_string(inner),
        _ => Err("mapping keys must be scalars".to_string()),
    }
}

/// Convert JSON into a YAML value, translating `{ "__jsExpr": ... }` nodes
/// into tagged `!!js` scalars (the long tag form on dump).
pub fn json_to_yaml(value: &Value) -> serde_yaml::Value {
    match value {
        Value::Null => serde_yaml::Value::Null,
        Value::Bool(value) => serde_yaml::Value::Bool(*value),
        Value::Number(value) => {
            if let Some(int) = value.as_i64() {
                serde_yaml::Value::Number(int.into())
            } else if let Some(float) = value.as_f64() {
                serde_yaml::Value::Number(float.into())
            } else {
                serde_yaml::Value::Null
            }
        }
        Value::String(value) => serde_yaml::Value::String(value.clone()),
        Value::Array(list) => serde_yaml::Value::Sequence(list.iter().map(json_to_yaml).collect()),
        Value::Object(map) => {
            if map.len() == 1
                && let Some(expr) = map.get("__jsExpr").and_then(|v| v.as_str())
            {
                return serde_yaml::Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
                    tag: serde_yaml::value::Tag::new("tag:yaml.org,2002:js"),
                    value: serde_yaml::Value::String(expr.to_string()),
                }));
            }
            let mut result = serde_yaml::Mapping::new();
            for (key, item) in map {
                result.insert(serde_yaml::Value::String(key.clone()), json_to_yaml(item));
            }
            serde_yaml::Value::Mapping(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_tag_parses_to_expr_node() {
        let json = parse_yaml("!!js '1 + 1'").unwrap();
        assert_eq!(json, json!({ "__jsExpr": "1 + 1" }));
        // long form also recognized
        let long = parse_yaml("!<tag:yaml.org,2002:js> '1 + 1'").unwrap();
        assert_eq!(long, json!({ "__jsExpr": "1 + 1" }));
        // plain scalar form (no quotes)
        let plain = parse_yaml("a: !!js process.env.DSH_MODEL ?? 'x'\n").unwrap();
        assert_eq!(
            plain,
            json!({ "a": { "__jsExpr": "process.env.DSH_MODEL ?? 'x'" } })
        );
        // block scalar form
        let block = parse_yaml("a: !!js >-\n  process.cwd()\n").unwrap();
        assert_eq!(block, json!({ "a": { "__jsExpr": "process.cwd()" } }));
    }

    #[test]
    fn nested_yaml_round_trips_through_json() {
        let json =
            parse_yaml("a: 1\nb:\n  - true\n  - x\nc:\n  d: !!js 'process.env.FOO'\n").unwrap();
        assert_eq!(
            json,
            json!({ "a": 1, "b": [true, "x"], "c": { "d": { "__jsExpr": "process.env.FOO" } } })
        );
        let back = json_to_yaml(&json);
        let text = serde_yaml::to_string(&back).unwrap();
        // serde_yaml dumps the verbatim tag `!tag:yaml.org,2002:js`
        // (semantically identical to the `!!js` shorthand).
        assert!(text.contains("2002:js"), "got: {text}");
        assert!(text.contains("process.env.FOO"), "got: {text}");
    }

    #[test]
    fn core_schema_scalars_stay_typed() {
        let json = parse_yaml("i: 5\nf: 1.5\nb: true\nn: null\ns: hi\n").unwrap();
        assert_eq!(
            json,
            json!({ "i": 5, "f": 1.5, "b": true, "n": null, "s": "hi" })
        );
    }
}
