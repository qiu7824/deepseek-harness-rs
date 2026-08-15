//! Enforced JSON Schema subset shared by tool outputs, generated Code Mode
//! types, subagents, and workflows. Rust port of
//! `packages/core/tools/src/json-schema.ts`.
//!
//! # Deviations
//!
//! - Schemas and validated values are `serde_json::Value`: the TS
//!   realm-boundary guards (`isPlainJsonRecord`, `isPlainJsonArray`,
//!   hostile-getter containment) collapse to the type system — a
//!   `serde_json::Value` is lossless JSON by construction, has no exotic
//!   prototypes, getters, sparse arrays, `NaN`/`Infinity`, or cycles.
//! - `assertSupportedJsonSchema`/`assertObjectJsonSchema` return
//!   `Result<(), JsonSchemaError>` instead of asserting.
//! - The `is circular` / `must be a lossless JSON value` diagnostics are
//!   unreachable and retained only in the shared message vocabulary.

use serde_json::Value as JsonValue;

/// Scalar JSON values supported by `enum` and `const`.
pub type JsonSchemaScalar = JsonValue;

/// One raw JSON Schema node in the enforced subset (any JSON root).
pub type JsonSchemaNode = JsonValue;

/// A consumer-constrained object-rooted schema.
pub type ObjectJsonSchema = JsonValue;

/// Thrown when a raw schema falls outside the enforced subset (TS
/// `JsonSchemaError`, code `UNSUPPORTED_SCHEMA`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSchemaError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Individual schema violations in walk order.
    pub violations: Vec<String>,
}

impl JsonSchemaError {
    pub fn new(violations: Vec<String>) -> Self {
        Self { code: "UNSUPPORTED_SCHEMA", violations }
    }
}

impl std::fmt::Display for JsonSchemaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unsupported JSON schema: {}", self.violations.join("; "))
    }
}

impl std::error::Error for JsonSchemaError {}

const CONSTRAINT_KEYWORDS: [&str; 8] = [
    "type", "oneOf", "properties", "required", "additionalProperties", "items", "enum", "const",
];
const ANNOTATION_KEYWORDS: [&str; 4] = ["description", "title", "default", "examples"];
const SCHEMA_TYPES: [&str; 7] = ["object", "array", "string", "number", "integer", "boolean", "null"];
const ONE_OF_SIBLING_KEYWORDS: [&str; 6] =
    ["properties", "required", "additionalProperties", "items", "enum", "const"];

fn is_schema_record(value: &JsonValue) -> bool {
    value.is_object()
}

fn is_plain_json_array(value: &JsonValue) -> bool {
    value.is_array()
}

/// Lossless finite JSON number, excluding negative zero.
fn is_json_number(value: &JsonValue) -> bool {
    if !value.is_number() {
        return false;
    }
    match value.as_f64() {
        Some(number) => number.is_finite() && !number.is_sign_negative(),
        None => true, // u64/i64 forms are finite by construction
    }
}

fn is_integer(value: &JsonValue) -> bool {
    if !value.is_number() {
        return false;
    }
    match value.as_f64() {
        Some(number) => number.is_finite() && number.fract() == 0.0,
        None => true,
    }
}

/// Whether a scalar is valid for one declared schema type.
fn scalar_matches(schema_type: &str, value: &JsonValue) -> bool {
    match schema_type {
        "string" => value.is_string(),
        "number" => is_json_number(value),
        "integer" => is_integer(value),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        other => panic!("forged schema type {other:?}"),
    }
}

fn has_own(node: &JsonValue, key: &str) -> bool {
    node.get(key).is_some()
}

/// Deferred work for the stack-safe raw-schema walk.
enum SchemaTask<'a> {
    Enter { node: &'a JsonValue, path: String },
    OneOfTail { node: &'a JsonValue, path: String },
    ObjectTail { node: &'a JsonValue, path: String, properties: Option<&'a JsonValue> },
}

/// Validate object-only fields after its property schemas have been visited.
fn check_object_schema_tail(
    node: &JsonValue,
    path: &str,
    properties: Option<&JsonValue>,
    violations: &mut Vec<String>,
) {
    let has_required = has_own(node, "required");
    let required = node.get("required");
    if has_required {
        let valid = required.is_some_and(|required| {
            required
                .as_array()
                .is_some_and(|entries| entries.iter().all(|entry| entry.is_string()))
        });
        if !valid {
            violations.push(format!("{path}.required must be an array of strings"));
        } else {
            let declared: &JsonValue = match properties {
                Some(properties) if properties.is_object() => properties,
                _ => &JsonValue::Null,
            };
            for key in required.expect("validated").as_array().expect("validated") {
                let key = key.as_str().expect("validated");
                if !has_own(declared, key) {
                    violations.push(format!(
                        "{path}.required names \"{key}\" which is not in properties"
                    ));
                }
            }
        }
    }
    if has_own(node, "additionalProperties") && !node["additionalProperties"].is_boolean() {
        violations.push(format!("{path}.additionalProperties must be a boolean"));
    }
}

/// Collect every violation for one raw schema tree without using the
/// recursive call stack.
fn check_schema_node(root: &JsonValue, root_path: &str, violations: &mut Vec<String>) {
    let mut tasks: Vec<SchemaTask<'_>> =
        vec![SchemaTask::Enter { node: root, path: root_path.to_string() }];
    while let Some(task) = tasks.pop() {
        match task {
            SchemaTask::OneOfTail { node, path } => {
                for key in ONE_OF_SIBLING_KEYWORDS {
                    if has_own(node, key) {
                        violations.push(format!("{path}.{key} is not supported beside oneOf"));
                    }
                }
            }
            SchemaTask::ObjectTail { node, path, properties } => {
                check_object_schema_tail(node, &path, properties, violations);
            }
            SchemaTask::Enter { node, path } => {
                if !is_schema_record(node) {
                    violations.push(format!("{path} must be a schema object"));
                    continue;
                }
                let object = node.as_object().expect("checked");

                for key in object.keys() {
                    if CONSTRAINT_KEYWORDS.contains(&key.as_str()) {
                        continue;
                    }
                    if ANNOTATION_KEYWORDS.contains(&key.as_str()) {
                        continue;
                    }
                    violations.push(format!(
                        "{path}.{key} is not a supported keyword (subset: type/oneOf/properties/required/additionalProperties/items/enum/const + annotations)"
                    ));
                }
                if has_own(node, "description") && !node["description"].is_string() {
                    violations.push(format!("{path}.description must be a string"));
                }
                if has_own(node, "title") && !node["title"].is_string() {
                    violations.push(format!("{path}.title must be a string"));
                }

                let has_type = has_own(node, "type");
                let has_one_of = has_own(node, "oneOf");
                if has_type && has_one_of {
                    violations.push(format!("{path} cannot declare both type and oneOf"));
                    continue;
                }
                if !has_type && !has_one_of {
                    for key in ONE_OF_SIBLING_KEYWORDS {
                        if has_own(node, key) {
                            violations.push(format!("{path}.{key} requires type or oneOf"));
                        }
                    }
                    continue;
                }

                if has_one_of {
                    let one_of = &node["oneOf"];
                    tasks.push(SchemaTask::OneOfTail { node, path: path.clone() });
                    let valid = is_plain_json_array(one_of)
                        && one_of.as_array().is_some_and(|branches| branches.len() >= 2);
                    if !valid {
                        violations.push(format!(
                            "{path}.oneOf must be an array of at least two schemas"
                        ));
                    } else {
                        let branches = one_of.as_array().expect("validated");
                        for index in (0..branches.len()).rev() {
                            tasks.push(SchemaTask::Enter {
                                node: &branches[index],
                                path: format!("{path}.oneOf[{index}]"),
                            });
                        }
                    }
                    continue;
                }

                let type_value = &node["type"];
                let schema_type = match type_value {
                    JsonValue::String(schema_type) if SCHEMA_TYPES.contains(&schema_type.as_str()) => {
                        schema_type.clone()
                    }
                    JsonValue::Array(_) => {
                        violations.push(format!(
                            "{path}.type must be a single type string (type arrays are not supported)"
                        ));
                        continue;
                    }
                    _ => {
                        violations.push(format!(
                            "{path}.type must be one of {}",
                            SCHEMA_TYPES.join("/")
                        ));
                        continue;
                    }
                };

                let allowed_for: [(&str, &[&str]); 6] = [
                    ("properties", &["object"]),
                    ("required", &["object"]),
                    ("additionalProperties", &["object"]),
                    ("items", &["array"]),
                    ("enum", &["string", "number", "integer", "boolean", "null"]),
                    ("const", &["string", "number", "integer", "boolean", "null"]),
                ];
                for (key, types) in allowed_for {
                    if has_own(node, key) && !types.contains(&schema_type.as_str()) {
                        violations.push(format!(
                            "{path}.{key} is not supported on type \"{schema_type}\""
                        ));
                    }
                }

                match schema_type.as_str() {
                    "object" => {
                        let properties = node.get("properties");
                        tasks.push(SchemaTask::ObjectTail {
                            node,
                            path: path.clone(),
                            properties,
                        });
                        if has_own(node, "properties") {
                            if !is_schema_record(properties.expect("present")) {
                                violations.push(format!("{path}.properties must be an object of schemas"));
                            } else {
                                let entries: Vec<(String, &JsonValue)> = properties
                                    .expect("present")
                                    .as_object()
                                    .expect("checked")
                                    .iter()
                                    .map(|(key, value)| (key.clone(), value))
                                    .collect();
                                for index in (0..entries.len()).rev() {
                                    let (key, child) = &entries[index];
                                    tasks.push(SchemaTask::Enter {
                                        node: child,
                                        path: format!("{path}.properties.{key}"),
                                    });
                                }
                            }
                        }
                    }
                    "array" => {
                        if has_own(node, "items") {
                            tasks.push(SchemaTask::Enter {
                                node: &node["items"],
                                path: format!("{path}.items"),
                            });
                        }
                    }
                    "string" | "number" | "integer" | "boolean" | "null" => {
                        let has_enum = has_own(node, "enum");
                        let allowed = node.get("enum");
                        let enum_valid = has_enum
                            && allowed.is_some_and(|allowed| {
                                is_plain_json_array(allowed)
                                    && allowed
                                        .as_array()
                                        .is_some_and(|entries| {
                                            !entries.is_empty()
                                                && entries
                                                    .iter()
                                                    .all(|entry| scalar_matches(&schema_type, entry))
                                        })
                            });
                        if has_enum && !enum_valid {
                            violations.push(format!(
                                "{path}.enum must be a non-empty array of {schema_type} values"
                            ));
                        }
                        let has_const = has_own(node, "const");
                        let declared_const = node.get("const");
                        let const_valid = has_const
                            && declared_const.is_some_and(|value| scalar_matches(&schema_type, value));
                        if has_const {
                            if !const_valid {
                                violations.push(format!("{path}.const must be a {schema_type} value"));
                            } else if enum_valid {
                                let entries = allowed.expect("present").as_array().expect("validated");
                                let const_value = declared_const.expect("present");
                                if !entries.iter().any(|entry| entry == const_value) {
                                    violations.push(format!(
                                        "{path}.const must be one of {path}.enum when both are declared"
                                    ));
                                }
                            }
                        }
                    }
                    other => panic!("forged schema type {other:?}"),
                }
            }
        }
    }
}

/// Assert that an arbitrary raw schema uses only the enforced subset.
pub fn assert_supported_json_schema(schema: &JsonValue) -> Result<(), JsonSchemaError> {
    let mut violations = Vec::new();
    check_schema_node(schema, "schema", &mut violations);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(JsonSchemaError::new(violations))
    }
}

/// Assert the enforced subset plus the object-root constraint retained by
/// subagent and workflow structured outputs.
pub fn assert_object_json_schema(schema: &JsonValue) -> Result<(), JsonSchemaError> {
    let mut violations = Vec::new();
    check_schema_node(schema, "schema", &mut violations);
    if violations.is_empty()
        && !(is_schema_record(schema)
            && has_own(schema, "type")
            && schema["type"].as_str() == Some("object"))
    {
        violations.push(
            "schema.type must be \"object\" (structured output is object-rooted)".to_string(),
        );
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(JsonSchemaError::new(violations))
    }
}

/// Root-aware diagnostic path for the parameter validator's empty sentinel.
fn diagnostic_path(path: &str) -> String {
    if path.is_empty() {
        "arguments".to_string()
    } else {
        path.to_string()
    }
}

/// Append one object property without a leading dot at an implicit root.
fn property_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// One child evaluation deferred by a container or exact-one union frame.
struct ValueChild<'a> {
    node: &'a JsonValue,
    value: &'a JsonValue,
    path: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    OneOf,
    Object,
    Array,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FramePhase {
    Start,
    Children,
}

/// Explicit call frame for stack-safe schema-value validation.
struct ValueFrame<'a> {
    node: &'a JsonValue,
    value: &'a JsonValue,
    path: String,
    phase: FramePhase,
    kind: Option<FrameKind>,
    children: Vec<ValueChild<'a>>,
    child_index: usize,
    violations: Vec<String>,
    tail_violations: Vec<String>,
    matches: usize,
}

/// Validate one scalar node after its primitive type check.
fn check_scalar_value(node: &JsonValue, value: &JsonValue, path: &str) -> Vec<String> {
    if let Some(allowed) = node.get("enum") {
        let entries = allowed.as_array().expect("asserted enum");
        if !entries.iter().any(|entry| entry == value) {
            return vec![format!(
                "\"{}\" must be one of {}",
                diagnostic_path(path),
                serde_json::to_string(allowed).expect("lossless JSON")
            )];
        }
    }
    if let Some(const_value) = node.get("const") {
        if value != const_value {
            return vec![format!(
                "\"{}\" must be {}",
                diagnostic_path(path),
                serde_json::to_string(const_value).expect("lossless JSON")
            )];
        }
    }
    Vec::new()
}

/// Complete one validation frame: pop it and deliver its result to the
/// parent (or the root).
fn finish_frame(
    frames: &mut Vec<ValueFrame<'_>>,
    root_result: &mut Option<Vec<String>>,
    result: Vec<String>,
) {
    frames.pop();
    match frames.last_mut() {
        None => *root_result = Some(result),
        Some(parent) if parent.kind == Some(FrameKind::OneOf) => {
            if result.is_empty() {
                parent.matches += 1;
            }
        }
        Some(parent) => parent.violations.extend(result),
    }
}

/// Validate one trusted schema/value pair with explicit frames rather than
/// recursive calls.
fn check_value(schema: &JsonValue, value: &JsonValue, path: &str) -> Vec<String> {
    let mut frames: Vec<ValueFrame<'_>> = vec![ValueFrame {
        node: schema,
        value,
        path: path.to_string(),
        phase: FramePhase::Start,
        kind: None,
        children: Vec::new(),
        child_index: 0,
        violations: Vec::new(),
        tail_violations: Vec::new(),
        matches: 0,
    }];
    let mut root_result: Option<Vec<String>> = None;

    loop {
        let Some(frame) = frames.last_mut() else {
            break;
        };
        if frame.phase == FramePhase::Children {
            if frame.child_index < frame.children.len() {
                let (child_node, child_value, child_path) = {
                    let child = &frame.children[frame.child_index];
                    frame.child_index += 1;
                    (child.node, child.value, child.path.clone())
                };
                frames.push(ValueFrame {
                    node: child_node,
                    value: child_value,
                    path: child_path,
                    phase: FramePhase::Start,
                    kind: None,
                    children: Vec::new(),
                    child_index: 0,
                    violations: Vec::new(),
                    tail_violations: Vec::new(),
                    matches: 0,
                });
                continue;
            }
            if frame.kind == Some(FrameKind::OneOf) {
                let result = if frame.matches == 1 {
                    Vec::new()
                } else {
                    vec![format!(
                        "\"{}\" must match exactly one oneOf branch (matched {})",
                        diagnostic_path(&frame.path),
                        frame.matches
                    )]
                };
                finish_frame(&mut frames, &mut root_result, result);
                continue;
            }
            let mut result = std::mem::take(&mut frame.violations);
            result.extend(std::mem::take(&mut frame.tail_violations));
            // serde_json values are lossless by construction: the TS
            // lossy-object/array containment collapses to success.
            finish_frame(&mut frames, &mut root_result, result);
            continue;
        }

        let node_type = frame.node.get("type").and_then(JsonValue::as_str).map(str::to_string);
        let one_of = frame.node.get("oneOf");
        if let Some(one_of) = one_of {
            frame.kind = Some(FrameKind::OneOf);
            frame.children = one_of
                .as_array()
                .expect("asserted oneOf")
                .iter()
                .map(|branch| ValueChild { node: branch, value: frame.value, path: frame.path.clone() })
                .collect();
            frame.child_index = 0;
            frame.matches = 0;
            frame.phase = FramePhase::Children;
            continue;
        }
        let Some(node_type) = node_type else {
            // Annotation-only schema: every serde_json value is lossless.
            finish_frame(&mut frames, &mut root_result, Vec::new());
            continue;
        };

        match node_type.as_str() {
            "object" => {
                if !frame.value.is_object() {
                    let message = format!("\"{}\" must be an object", diagnostic_path(&frame.path));
                    finish_frame(&mut frames, &mut root_result, vec![message]);
                    continue;
                }
                let properties = frame.node.get("properties").and_then(JsonValue::as_object);
                let violations: Vec<String> = match frame.node.get("required") {
                    Some(JsonValue::Array(required)) => required
                        .iter()
                        .filter_map(|key| {
                            let key = key.as_str().expect("asserted required");
                            if frame.value.get(key).is_some() {
                                None
                            } else {
                                Some(format!(
                                    "missing required property \"{}\"",
                                    property_path(&frame.path, key)
                                ))
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let children: Vec<ValueChild<'_>> = match properties {
                    Some(properties) => properties
                        .iter()
                        .filter_map(|(key, child)| {
                            frame.value.get(key).map(|value| ValueChild {
                                node: child,
                                value,
                                path: property_path(&frame.path, key),
                            })
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let tail_violations: Vec<String> = if frame.node.get("additionalProperties")
                    == Some(&JsonValue::Bool(false))
                {
                    frame
                        .value
                        .as_object()
                        .expect("checked")
                        .keys()
                        .filter(|key| {
                            !properties.is_some_and(|properties| properties.contains_key(*key))
                        })
                        .map(|key| {
                            format!(
                                "\"{}\" is not a declared property (additionalProperties: false)",
                                property_path(&frame.path, key)
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                frame.kind = Some(FrameKind::Object);
                frame.children = children;
                frame.child_index = 0;
                frame.violations = violations;
                frame.tail_violations = tail_violations;
                frame.phase = FramePhase::Children;
            }
            "array" => {
                if !frame.value.is_array() {
                    let message = format!("\"{}\" must be an array", diagnostic_path(&frame.path));
                    finish_frame(&mut frames, &mut root_result, vec![message]);
                    continue;
                }
                let items = frame.node.get("items");
                let children: Vec<ValueChild<'_>> = match items {
                    Some(items) => frame
                        .value
                        .as_array()
                        .expect("checked")
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| ValueChild {
                            node: items,
                            value: entry,
                            path: format!("{}[{index}]", frame.path),
                        })
                        .collect(),
                    None => Vec::new(),
                };
                frame.kind = Some(FrameKind::Array);
                frame.children = children;
                frame.child_index = 0;
                frame.violations = Vec::new();
                frame.phase = FramePhase::Children;
            }
            "string" => {
                let result = if frame.value.is_string() {
                    check_scalar_value(frame.node, frame.value, &frame.path)
                } else {
                    vec![format!("\"{}\" must be a string", diagnostic_path(&frame.path))]
                };
                finish_frame(&mut frames, &mut root_result, result);
            }
            "number" => {
                let result = if !frame.value.is_number() {
                    vec![format!("\"{}\" must be a number", diagnostic_path(&frame.path))]
                } else if !is_json_number(frame.value) {
                    vec![format!(
                        "\"{}\" must be a finite JSON number",
                        diagnostic_path(&frame.path)
                    )]
                } else {
                    check_scalar_value(frame.node, frame.value, &frame.path)
                };
                finish_frame(&mut frames, &mut root_result, result);
            }
            "integer" => {
                let result = if !is_integer(frame.value) {
                    vec![format!("\"{}\" must be an integer", diagnostic_path(&frame.path))]
                } else {
                    check_scalar_value(frame.node, frame.value, &frame.path)
                };
                finish_frame(&mut frames, &mut root_result, result);
            }
            "boolean" => {
                let result = if frame.value.is_boolean() {
                    check_scalar_value(frame.node, frame.value, &frame.path)
                } else {
                    vec![format!("\"{}\" must be a boolean", diagnostic_path(&frame.path))]
                };
                finish_frame(&mut frames, &mut root_result, result);
            }
            "null" => {
                let result = if frame.value.is_null() {
                    check_scalar_value(frame.node, frame.value, &frame.path)
                } else {
                    vec![format!("\"{}\" must be null", diagnostic_path(&frame.path))]
                };
                finish_frame(&mut frames, &mut root_result, result);
            }
            other => panic!("forged schema type {other:?}"),
        }
    }

    root_result.unwrap_or_default()
}

/// Validate a candidate value against an asserted raw schema. The function
/// is total for arbitrary values and returns path-qualified violations.
pub fn validate_json_schema_value(
    schema: &JsonValue,
    value: &JsonValue,
    path: &str,
) -> Vec<String> {
    check_value(schema, value, path)
}
