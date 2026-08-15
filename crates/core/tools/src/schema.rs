//! Unified JSON-value schema DSL, compilation, and validation helper. Rust
//! port of `packages/core/tools/src/schema.ts` (the `defineTool` typed
//! helper arrives with the registry milestone; this module delivers the
//! compile/validate layer).
//!
//! # Deviations
//!
//! - The TS DSL is a structurally-typed union with compile-time
//!   `InferValue`/`InferArgs` inference. Rust models it as the closed
//!   [`ValueSchemaSpec`] enum, so the TS runtime-forged-form rejections
//!   (unknown keys, `type` beside `oneOf`, non-boolean
//!   `additionalProperties`, mismatched scalar `enum`/`const` element
//!   types, `required: false`, symbol keys) collapse into the type
//!   system. Retained runtime checks: `oneOf` needs at least two
//!   branches, `enum` must be non-empty, and the compiled projection is
//!   re-asserted through [`assert_supported_json_schema`] exactly like the
//!   TS pipeline.
//! - Integer literals use `i64` (the TS `number` restricts to integer
//!   JSON values; `i64` is its wire image).
//! - Cyclic author schemas are unrepresentable in the owned enum.

use indexmap::IndexMap;
use serde_json::{Value as JsonValue, json};

use crate::json_schema::{
    JsonSchemaError, JsonSchemaNode, assert_supported_json_schema, validate_json_schema_value,
};

/// Annotation keywords shared by every author-facing schema node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueSchemaAnnotations {
    /// Human-readable description projected into JSON Schema.
    pub description: Option<String>,
    /// Human-readable title projected into JSON Schema.
    pub title: Option<String>,
    /// Non-validating default annotation; lossless JSON data.
    pub default: Option<JsonValue>,
    /// Non-validating examples annotation; lossless JSON data.
    pub examples: Option<JsonValue>,
}

impl ValueSchemaAnnotations {
    fn apply(&self, node: &mut serde_json::Map<String, JsonValue>) {
        if let Some(description) = &self.description {
            node.insert("description".to_string(), json!(description));
        }
        if let Some(title) = &self.title {
            node.insert("title".to_string(), json!(title));
        }
        if let Some(default) = &self.default {
            node.insert("default".to_string(), default.clone());
        }
        if let Some(examples) = &self.examples {
            node.insert("examples".to_string(), examples.clone());
        }
    }
}

/// String value schema with type-correct literal constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct StringValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub enum_: Option<Vec<String>>,
    pub const_: Option<String>,
}

/// Finite JSON-number schema with type-correct literal constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub enum_: Option<Vec<f64>>,
    pub const_: Option<f64>,
}

/// Integer schema with type-correct literal constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct IntegerValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub enum_: Option<Vec<i64>>,
    pub const_: Option<i64>,
}

/// Boolean value schema with type-correct literal constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct BooleanValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub enum_: Option<Vec<bool>>,
    pub const_: Option<bool>,
}

/// Null value schema.
#[derive(Debug, Clone, PartialEq)]
pub struct NullValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
}

/// Array value schema; omitted `items` accepts any lossless JSON item.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub items: Option<Box<ValueSchemaSpec>>,
}

/// Explicit object value schema. Openness is mandatory so a nested or
/// output object never acquires an accidental JSON Schema default.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub properties: Option<ParameterSchemaSpec>,
    pub additional_properties: bool,
}

/// Author-only unconstrained lossless JSON node.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
}

/// Exact-one union schema; at least two branches are required.
#[derive(Debug, Clone, PartialEq)]
pub struct OneOfValueSchemaSpec {
    pub annotations: ValueSchemaAnnotations,
    pub branches: Vec<ValueSchemaSpec>,
}

/// One author-facing schema for any lossless JSON value root.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSchemaSpec {
    String(StringValueSchemaSpec),
    Number(NumberValueSchemaSpec),
    Integer(IntegerValueSchemaSpec),
    Boolean(BooleanValueSchemaSpec),
    Null(NullValueSchemaSpec),
    Array(ArrayValueSchemaSpec),
    Object(ObjectValueSchemaSpec),
    Json(JsonValueSchemaSpec),
    OneOf(OneOfValueSchemaSpec),
}

impl ValueSchemaSpec {
    pub fn annotations(&self) -> &ValueSchemaAnnotations {
        match self {
            ValueSchemaSpec::String(spec) => &spec.annotations,
            ValueSchemaSpec::Number(spec) => &spec.annotations,
            ValueSchemaSpec::Integer(spec) => &spec.annotations,
            ValueSchemaSpec::Boolean(spec) => &spec.annotations,
            ValueSchemaSpec::Null(spec) => &spec.annotations,
            ValueSchemaSpec::Array(spec) => &spec.annotations,
            ValueSchemaSpec::Object(spec) => &spec.annotations,
            ValueSchemaSpec::Json(spec) => &spec.annotations,
            ValueSchemaSpec::OneOf(spec) => &spec.annotations,
        }
    }
}

/// One implicit parameter-root property, optionally required.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterPropertySpec {
    pub schema: ValueSchemaSpec,
    /// `required: true` marks the property required; omission leaves it
    /// optional (the TS `true`-or-absent annotation).
    pub required: bool,
}

/// Tool parameter schema: an implicit open object root with per-property
/// requiredness. Insertion order is preserved in the JSON projection.
pub type ParameterSchemaSpec = IndexMap<String, ParameterPropertySpec>;

/// Raw JSON Schema projection of the implicit parameter object.
pub type ParameterJsonSchema = JsonSchemaNode;

/// Invalid model-generated arguments for a typed tool (TS `ToolArgsError`,
/// code `INVALID_ARGS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArgsError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Individual violations in schema-walk order.
    pub violations: Vec<String>,
}

impl ToolArgsError {
    pub fn new(violations: Vec<String>) -> Self {
        Self { code: "INVALID_ARGS", violations }
    }
}

impl std::fmt::Display for ToolArgsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid arguments: {}", self.violations.join("; "))
    }
}

impl std::error::Error for ToolArgsError {}

fn author_error(message: &str) -> JsonSchemaError {
    JsonSchemaError::new(vec![message.to_string()])
}

/// Compile one author node into its raw JSON Schema projection. The
/// type-closed enum removes most TS runtime author checks; the returned
/// node is re-asserted by the caller.
fn compile_value(spec: &ValueSchemaSpec, node: &mut serde_json::Map<String, JsonValue>) -> Result<(), JsonSchemaError> {
    match spec {
        ValueSchemaSpec::String(StringValueSchemaSpec { annotations, enum_, const_ }) => {
            node.insert("type".to_string(), json!("string"));
            annotations.apply(node);
            if let Some(entries) = enum_ {
                if entries.is_empty() {
                    return Err(author_error("schema.enum must be a non-empty array of string values"));
                }
                node.insert("enum".to_string(), json!(entries));
            }
            if let Some(value) = const_ {
                node.insert("const".to_string(), json!(value));
            }
        }
        ValueSchemaSpec::Number(NumberValueSchemaSpec { annotations, enum_, const_ }) => {
            node.insert("type".to_string(), json!("number"));
            annotations.apply(node);
            if let Some(entries) = enum_ {
                if entries.is_empty() {
                    return Err(author_error("schema.enum must be a non-empty array of number values"));
                }
                let rendered: Vec<JsonValue> = entries
                    .iter()
                    .map(|entry| serde_json::Number::from_f64(*entry).map(JsonValue::Number).expect("finite JSON number"))
                    .collect();
                node.insert("enum".to_string(), JsonValue::Array(rendered));
            }
            if let Some(value) = const_ {
                node.insert(
                    "const".to_string(),
                    JsonValue::Number(serde_json::Number::from_f64(*value).expect("finite JSON number")),
                );
            }
        }
        ValueSchemaSpec::Integer(IntegerValueSchemaSpec { annotations, enum_, const_ }) => {
            node.insert("type".to_string(), json!("integer"));
            annotations.apply(node);
            if let Some(entries) = enum_ {
                if entries.is_empty() {
                    return Err(author_error("schema.enum must be a non-empty array of integer values"));
                }
                node.insert("enum".to_string(), json!(entries));
            }
            if let Some(value) = const_ {
                node.insert("const".to_string(), json!(value));
            }
        }
        ValueSchemaSpec::Boolean(BooleanValueSchemaSpec { annotations, enum_, const_ }) => {
            node.insert("type".to_string(), json!("boolean"));
            annotations.apply(node);
            if let Some(entries) = enum_ {
                if entries.is_empty() {
                    return Err(author_error("schema.enum must be a non-empty array of boolean values"));
                }
                node.insert("enum".to_string(), json!(entries));
            }
            if let Some(value) = const_ {
                node.insert("const".to_string(), json!(value));
            }
        }
        ValueSchemaSpec::Null(NullValueSchemaSpec { annotations }) => {
            node.insert("type".to_string(), json!("null"));
            annotations.apply(node);
        }
        ValueSchemaSpec::Array(ArrayValueSchemaSpec { annotations, items }) => {
            node.insert("type".to_string(), json!("array"));
            annotations.apply(node);
            if let Some(items) = items {
                let mut item_node = serde_json::Map::new();
                compile_value(items, &mut item_node)?;
                node.insert("items".to_string(), JsonValue::Object(item_node));
            }
        }
        ValueSchemaSpec::Object(ObjectValueSchemaSpec { annotations, properties, additional_properties }) => {
            node.insert("type".to_string(), json!("object"));
            annotations.apply(node);
            node.insert("additionalProperties".to_string(), json!(additional_properties));
            if let Some(properties) = properties {
                let (property_nodes, required) = compile_property_map(properties)?;
                node.insert("properties".to_string(), JsonValue::Object(property_nodes));
                if !required.is_empty() {
                    node.insert("required".to_string(), json!(required));
                }
            }
        }
        ValueSchemaSpec::Json(JsonValueSchemaSpec { annotations }) => {
            // The author-only `json` node becomes an annotation-only schema.
            annotations.apply(node);
        }
        ValueSchemaSpec::OneOf(OneOfValueSchemaSpec { annotations, branches }) => {
            if branches.len() < 2 {
                return Err(author_error("schema.oneOf must be an array of at least two value schemas"));
            }
            annotations.apply(node);
            let mut rendered = Vec::with_capacity(branches.len());
            for branch in branches {
                let mut branch_node = serde_json::Map::new();
                compile_value(branch, &mut branch_node)?;
                rendered.push(JsonValue::Object(branch_node));
            }
            node.insert("oneOf".to_string(), JsonValue::Array(rendered));
        }
    }
    Ok(())
}

/// Compile one implicit property map, collecting per-property requiredness.
fn compile_property_map(
    properties: &ParameterSchemaSpec,
) -> Result<(serde_json::Map<String, JsonValue>, Vec<String>), JsonSchemaError> {
    let mut compiled = serde_json::Map::new();
    let mut required = Vec::new();
    for (key, property) in properties {
        let mut node = serde_json::Map::new();
        compile_value(&property.schema, &mut node)?;
        if property.required {
            required.push(key.clone());
        }
        compiled.insert(key.clone(), JsonValue::Object(node));
    }
    Ok((compiled, required))
}

/// Compile one author-facing value schema to the enforced raw JSON Schema
/// subset. The author-only `json` node becomes an annotation-only schema.
pub fn value_schema_spec_to_json_schema(spec: &ValueSchemaSpec) -> Result<JsonSchemaNode, JsonSchemaError> {
    let mut node = serde_json::Map::new();
    compile_value(spec, &mut node)?;
    let schema = JsonValue::Object(node);
    assert_supported_json_schema(&schema)?;
    Ok(schema)
}

/// Compile the implicit open parameter object into raw JSON Schema.
pub fn parameter_schema_spec_to_json_schema(
    spec: &ParameterSchemaSpec,
) -> Result<ParameterJsonSchema, JsonSchemaError> {
    let (properties, required) = compile_property_map(spec)?;
    let mut node = serde_json::Map::new();
    node.insert("type".to_string(), json!("object"));
    node.insert("properties".to_string(), JsonValue::Object(properties));
    if !required.is_empty() {
        node.insert("required".to_string(), json!(required));
    }
    let schema = JsonValue::Object(node);
    assert_supported_json_schema(&schema)?;
    Ok(schema)
}

/// Validate model-generated arguments against an implicit parameter
/// schema. Returns path-qualified violations; empty means valid.
pub fn validate_args(
    spec: &ParameterSchemaSpec,
    args: &JsonValue,
) -> Result<Vec<String>, JsonSchemaError> {
    let schema = parameter_schema_spec_to_json_schema(spec)?;
    Ok(validate_json_schema_value(&schema, args, ""))
}
